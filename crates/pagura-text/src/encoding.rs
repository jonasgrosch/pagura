//! Simple encodings and the glyph-name algorithm (9.6.5, 9.10.2, Annex D).
use crate::{GlyphText, repairs, report::Report, tables};
use pagura_core::{Diagnostic, Dictionary, Error, Limits, Meter, Name, Result, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;

pub(crate) type Encoding = [Option<Name>; 256];

#[derive(Debug)]
pub(crate) struct SimpleMapping {
    pub(crate) names: Encoding,
    /// Whether names are read as ZapfDingbats names first.
    dingbats: bool,
    /// Each code's Unicode from its glyph name, found on first use.
    unicode: [std::sync::OnceLock<Option<GlyphText>>; 256],
    /// Each code's program glyph, selected on first use: a function of the
    /// code, its name and its Unicode alone (9.6.5).
    pub(crate) selected: [std::sync::OnceLock<Option<pagura_font::GlyphId>>; 256],
}

/// The bytes of the Unicode text a glyph name of `length` bytes can map
/// to at most: four per name byte (a component of one byte maps to one
/// scalar, `uniXXXX` and `uXXXX` forms need more bytes than they produce).
pub(crate) const UNICODE_BYTES_PER_NAME_BYTE: usize = 4;

impl SimpleMapping {
    pub(crate) fn new(names: Encoding, limits: &Limits, base_font: &[u8]) -> Result<Self> {
        // A name's text is at most a few bytes per name byte, so charging the
        // names bounds both the work and the text.
        let mut work = Meter::new(limits.font_work, "simple encoding work budget exceeded");
        for name in names.iter().flatten() {
            work.charge(name.len())?;
        }
        Ok(Self {
            names,
            dingbats: is_dingbats(base_font),
            unicode: std::array::from_fn(|_| std::sync::OnceLock::new()),
            selected: std::array::from_fn(|_| std::sync::OnceLock::new()),
        })
    }

    /// The Unicode text of `code` from its glyph name (9.10.2), found when
    /// first asked for: decoding needs it for the codes a string uses only.
    pub(crate) fn unicode(&self, code: u8) -> Option<&GlyphText> {
        let index = usize::from(code);
        self.unicode
            .get(index)?
            .get_or_init(|| {
                let name = self.names.get(index)?.as_ref()?;
                unicode_with(name.as_bytes(), self.dingbats).map(GlyphText::from)
            })
            .as_ref()
    }
}

pub(crate) fn font_name<'a>(document: &'a Document, font: &'a Dictionary) -> ReadResult<&'a [u8]> {
    Ok(crate::objects::optional(document, font, b"BaseFont")?
        .and_then(Value::as_name)
        .map_or(b"", Name::as_bytes))
}

/// The predefined simple-font encodings of Annex D (Tables D.2, D.4–D.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "PDFDoc, Symbol and ZapfDingbats are Annex D reference tables checked by tests; \\
                  fonts take those encodings from their programs or providers"
    )
)]
pub(crate) enum BaseEncoding {
    Standard,
    MacRoman,
    WinAnsi,
    MacExpert,
    PdfDoc,
    Symbol,
    ZapfDingbats,
}

impl BaseEncoding {
    /// The encodings an `/Encoding` or `/BaseEncoding` name may select (Table 112).
    fn from_pdf_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"MacRomanEncoding" => Self::MacRoman,
            b"MacExpertEncoding" => Self::MacExpert,
            b"WinAnsiEncoding" => Self::WinAnsi,
            _ => return None,
        })
    }

    /// Any Annex D encoding by its table name.
    #[cfg(test)]
    pub(crate) fn from_table_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"StandardEncoding" => Self::Standard,
            b"MacRomanEncoding" => Self::MacRoman,
            b"WinAnsiEncoding" => Self::WinAnsi,
            b"MacExpertEncoding" => Self::MacExpert,
            b"PDFDocEncoding" => Self::PdfDoc,
            b"Symbol" => Self::Symbol,
            b"ZapfDingbats" => Self::ZapfDingbats,
            _ => return None,
        })
    }

    const fn table(self) -> &'static [(u8, &'static str)] {
        match self {
            Self::Standard => tables::STANDARD,
            Self::MacRoman => tables::MAC_ROMAN,
            Self::WinAnsi => tables::WIN_ANSI,
            Self::MacExpert => tables::MAC_EXPERT,
            Self::PdfDoc => tables::PDF_DOC,
            Self::Symbol => tables::SYMBOL,
            Self::ZapfDingbats => tables::ZAPF_DINGBATS,
        }
    }

    /// The code-to-name table, undefined codes absent.
    pub(crate) fn names(self) -> Encoding {
        let mut encoding = std::array::from_fn(|_| None);
        for &(code, name) in self.table() {
            encoding[usize::from(code)] = Some(Name::new(name.as_bytes()));
        }
        encoding
    }
}

/// The predefined encoding a name selects. An unknown name is the reported
/// repair `font.encoding-name`: the font's default encoding applies.
fn predefined(document: &Document, name: &Name) -> Result<Option<BaseEncoding>> {
    if let Some(encoding) = BaseEncoding::from_pdf_name(name.as_bytes()) {
        return Ok(Some(encoding));
    }
    Report::new(document).diagnostic(Diagnostic::new(
        repairs::ENCODING_NAME,
        "unknown simple-font encoding name; using the font's default encoding (Table 112)",
    ))?;
    Ok(None)
}

/// A simple font's `/Encoding` over `default`, the encoding that applies
/// without one: a predefined name, or a dictionary with an optional
/// `/BaseEncoding` and `/Differences` (9.6.5.1).
pub(crate) fn load(
    document: &Document,
    value: Option<&Value>,
    default: Encoding,
) -> ReadResult<Encoding> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = document.resolve(value)?;
    if let Some(name) = value.as_name() {
        return Ok(predefined(document, name)?.map_or(default, BaseEncoding::names));
    }
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::syntax("invalid font Encoding"))?;
    let mut encoding = if let Some(value) = dict.get(b"BaseEncoding") {
        let name = document
            .resolve(value)?
            .as_name()
            .ok_or_else(|| Error::syntax("invalid BaseEncoding"))?;
        predefined(document, name)?.map_or(default, BaseEncoding::names)
    } else {
        default
    };
    if let Some(value) = dict.get(b"Differences") {
        let array = document
            .resolve(value)?
            .as_array()
            .ok_or_else(|| Error::syntax("invalid Differences"))?;
        let mut code = None;
        for value in array {
            let value = document.resolve(value)?;
            if let Some(number) = value.as_integer() {
                code = Some(usize::from(u8::try_from(number).map_err(|_| {
                    Error::syntax("Differences code outside byte range")
                })?));
            } else if let Some(name) = value.as_name() {
                let slot = code
                    .and_then(|code| encoding.get_mut(code))
                    .ok_or_else(|| Error::syntax("Differences name without valid code"))?;
                *slot = Some(name.clone());
                code = code.map(|code| code + 1);
            } else {
                return Err(Error::syntax("invalid Differences element").into());
            }
        }
    }
    Ok(encoding)
}

/// AGL name decomposition: suffix removal, components, uniXXXX and uXXXX.
/// Unknown names remain unknown rather than inventing a Unicode value.
pub(crate) fn unicode(name: &[u8]) -> Option<String> {
    unicode_with(name, false)
}

pub(crate) fn unicode_for_font(name: &[u8], base_font: &[u8]) -> Option<String> {
    unicode_with(name, is_dingbats(base_font))
}

/// Whether a base font name is ZapfDingbats under the substitution aliases,
/// so its Annex D.6 glyph names apply.
fn is_dingbats(base_font: &[u8]) -> bool {
    crate::substitute::standard_name(base_font) == Some(crate::substitute::Standard14::ZapfDingbats)
}

fn unicode_with(name: &[u8], dingbats: bool) -> Option<String> {
    let stem = name.split(|byte| *byte == b'.').next()?;
    let mut result = String::new();
    for component in stem.split(|byte| *byte == b'_') {
        match component_unicode(component, dingbats) {
            Some(Component::Scalar(scalar)) => result.push(scalar),
            Some(Component::Sequence(text)) => result.push_str(&text),
            None => {}
        }
    }
    (!result.is_empty()).then_some(result)
}

enum Component {
    Scalar(char),
    Sequence(String),
}

fn component_unicode(component: &[u8], dingbats: bool) -> Option<Component> {
    let component = std::str::from_utf8(component).ok()?;
    if let Ok(index) = tables::UNICODE.binary_search_by_key(&component, |(name, _)| *name) {
        return Some(Component::Scalar(char::from_u32(tables::UNICODE[index].1)?));
    }
    if dingbats
        && let Ok(index) =
            tables::DINGBATS_UNICODE.binary_search_by_key(&component, |(name, _)| *name)
    {
        return Some(Component::Scalar(char::from_u32(
            tables::DINGBATS_UNICODE[index].1,
        )?));
    }
    if let Some(scalar) = cyrillic(component) {
        return Some(Component::Scalar(scalar));
    }
    if let Some(hex) = component.strip_prefix("uni") {
        if hex.is_empty() || !hex.len().is_multiple_of(4) || !upper_hex(hex) {
            return None;
        }
        if hex.len() == 4 {
            return Some(Component::Scalar(char::from_u32(
                u32::from_str_radix(hex, 16).ok()?,
            )?));
        }
        // Build one component atomically: a malformed scalar invalidates this
        // component, while other underscore components remain meaningful.
        let mut result = String::new();
        for chunk in hex.as_bytes().as_chunks::<4>().0 {
            let scalar = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            result.push(char::from_u32(scalar)?);
        }
        return Some(Component::Sequence(result));
    }
    let hex = component.strip_prefix('u')?;
    if !(4..=6).contains(&hex.len()) || !upper_hex(hex) {
        return None;
    }
    Some(Component::Scalar(char::from_u32(
        u32::from_str_radix(hex, 16).ok()?,
    )?))
}

// Russian AFII names from the AGL data: Yo/yo interrupt the two alphabetical
// runs. Other AFII scripts and unregistered numeric spellings remain unknown.
fn cyrillic(name: &str) -> Option<char> {
    let digits = name.strip_prefix("afii")?;
    if digits.len() != 5 {
        return None;
    }
    let number = digits.parse::<u32>().ok()?;
    char::from_u32(match number {
        10017..=10022 => 0x410 + number - 10017,
        10023 => 0x401,
        10024..=10049 => 0x416 + number - 10024,
        10065..=10070 => 0x430 + number - 10065,
        10071 => 0x451,
        10072..=10097 => 0x436 + number - 10072,
        _ => return None,
    })
}

fn upper_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{expected, integer, rows, text};
    use pagura_testkit::fixture;
    #[test]
    fn dingbats_names_require_a_recognized_font_family() {
        for &(name, _) in tables::DINGBATS_UNICODE {
            assert_eq!(unicode(name.as_bytes()), None, "{name}");
            assert_eq!(unicode_for_font(name.as_bytes(), b"Rect"), None, "{name}");
        }
        for font in [
            b"ZapfDingbats".as_slice(),
            b"ABCDEF+ZapfDingbats-Bold",
            b"Dingbats",
        ] {
            assert_eq!(
                unicode_for_font(b"a12_a19.alt", font).as_deref(),
                Some("☞✓")
            );
        }
        for font in [b"Rect".as_slice(), b"NotZapfDingbats", b"ZapfDingbatsExtra"] {
            assert_eq!(unicode_for_font(b"a12_a19.alt", font), None);
            assert_eq!(
                unicode_for_font(b"A_uni0042.alt", font).as_deref(),
                Some("AB")
            );
        }
        assert_eq!(
            unicode_for_font(b"a12_a19.alt", b"ZapfDingbats").as_deref(),
            Some("☞✓")
        );
    }

    #[test]
    fn glyph_name_tables_are_sorted_for_binary_search() {
        assert!(tables::UNICODE.is_sorted_by_key(|(name, _)| *name));
        assert!(tables::DINGBATS_UNICODE.is_sorted_by_key(|(name, _)| *name));
    }

    #[test]
    fn names_and_encoding_exceptions() {
        assert_eq!(unicode(b"uni00410042_u1F600.alt").as_deref(), Some("AB😀"));
        assert_eq!(unicode(b"Euro_uni0041D800_B").as_deref(), Some("€B"));
        for name in [b"uniD800".as_slice(), b"u110000", b"uni00e9", b"unknown"] {
            assert_eq!(unicode(name), None);
        }
        assert_eq!(unicode(b"A__B").as_deref(), Some("AB"));
        let win = BaseEncoding::WinAnsi.names();
        assert_eq!(
            win[160].as_ref().map(Name::as_bytes),
            Some(b"space".as_slice())
        );
        assert_eq!(
            win[173].as_ref().map(Name::as_bytes),
            Some(b"hyphen".as_slice())
        );
        assert_eq!(
            win[129].as_ref().map(Name::as_bytes),
            Some(b"bullet".as_slice())
        );
        let mac = BaseEncoding::MacRoman.names();
        assert_eq!(
            mac[219].as_ref().map(Name::as_bytes),
            Some(b"currency".as_slice())
        );
    }
    #[test]
    fn base_encoding_vectors() -> Result<()> {
        for name in [
            "winansi",
            "macroman",
            "standard",
            "macexpert",
            "symbol",
            "dingbats",
            "pdfdoc",
            "table-winansi",
            "table-macroman",
        ] {
            let name = format!("encoding-{name}");
            let base = fixture("text", &format!("{name}.base"));
            let encoding = BaseEncoding::from_table_name(&base)
                .ok_or_else(|| Error::syntax("unknown base encoding"))?
                .names();
            for value in expected(&name)? {
                let row = rows(&value)?;
                let code = usize::try_from(integer(&row[0])?)
                    .map_err(|_| Error::syntax("invalid code"))?;
                let unicode = encoding[code]
                    .as_ref()
                    .and_then(|name| unicode_for_font(name.as_bytes(), &base));
                assert_eq!(unicode.as_deref(), text(&row[4]), "{name}: {code}");
            }
        }
        Ok(())
    }

    #[test]
    fn annex_code_name_vectors() -> Result<()> {
        for name in [
            "StandardEncoding",
            "MacRomanEncoding",
            "WinAnsiEncoding",
            "PDFDocEncoding",
            "MacExpertEncoding",
            "Symbol",
            "ZapfDingbats",
        ] {
            let encoding = BaseEncoding::from_table_name(name.as_bytes())
                .ok_or_else(|| Error::syntax("unknown base encoding"))?
                .names();
            for value in expected(&format!("annex-{name}"))? {
                let row = rows(&value)?;
                let code = usize::try_from(integer(&row[0])?)
                    .map_err(|_| Error::syntax("invalid code"))?;
                assert_eq!(
                    encoding[code].as_ref().map(Name::as_bytes),
                    row[1].as_string(),
                    "{name}: {code}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn glyph_name_known_answers() -> Result<()> {
        for value in expected("glyph-name-forms")? {
            let row = rows(&value)?;
            let name = row[0]
                .as_string()
                .ok_or_else(|| Error::syntax("invalid glyph name oracle"))?;
            assert_eq!(unicode(name).as_deref(), text(&row[1]), "{name:?}");
        }
        Ok(())
    }

    #[test]
    fn cyrillic_names_agree_with_all_sixty_six_independent_records() -> Result<()> {
        for value in expected("glyph-name-cyrillic")? {
            let row = value
                .as_array()
                .ok_or_else(|| Error::syntax("expected name row"))?;
            let name = row[0]
                .as_string()
                .ok_or_else(|| Error::syntax("expected glyph name"))?;
            assert_eq!(
                unicode(name).as_deref().map(str::as_bytes),
                row[1].as_string()
            );
        }
        for name in [
            b"afii010017".as_slice(),
            b"afii+0017",
            b"afii10016",
            b"afii10050",
            b"afii10098",
        ] {
            assert!(unicode(name).is_none());
        }
        Ok(())
    }
}
