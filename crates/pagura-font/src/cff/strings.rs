//! Standard strings, StandardEncoding and CFF encodings (TN 5176 sections
//! 10 and 12, appendices A and B).
use super::index::Index;
use super::predefined;
use crate::budget::Work;
use crate::read::{limit, syntax};
use pagura_core::bytes::Cursor;
use pagura_core::{Name, Position, Result};

mod supplements;

/// SIDs below this name predefined strings (TN 5176 Appendix A).
pub(crate) const STANDARD_STRINGS: usize = 391;
/// SIDs from this one up are reserved (TN 5176 section 10).
pub(crate) const RESERVED_SID: u16 = 65000;

/// Top DICT Encoding offsets that name predefined encodings (TN 5176
/// section 12); larger values are offsets of explicit encodings.
const STANDARD_ENCODING: usize = 0;
const EXPERT_ENCODING: usize = 1;
/// Explicit encoding formats (TN 5176 Tables 11 and 12) and the format bit
/// announcing supplements (Table 14).
const CODES: u8 = 0;
const RANGES: u8 = 1;
const FORMAT_MASK: u8 = 0x7f;
const HAS_SUPPLEMENTS: u8 = 0x80;

/// SIDs 0-228: the ISOAdobe charset (TN 5176 Appendix A).
const ISO_ADOBE: [&str; 229] = [
    ".notdef",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quoteright",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "quoteleft",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "exclamdown",
    "cent",
    "sterling",
    "fraction",
    "yen",
    "florin",
    "section",
    "currency",
    "quotesingle",
    "quotedblleft",
    "guillemotleft",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "endash",
    "dagger",
    "daggerdbl",
    "periodcentered",
    "paragraph",
    "bullet",
    "quotesinglbase",
    "quotedblbase",
    "quotedblright",
    "guillemotright",
    "ellipsis",
    "perthousand",
    "questiondown",
    "grave",
    "acute",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "dieresis",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "emdash",
    "AE",
    "ordfeminine",
    "Lslash",
    "Oslash",
    "OE",
    "ordmasculine",
    "ae",
    "dotlessi",
    "lslash",
    "oslash",
    "oe",
    "germandbls",
    "onesuperior",
    "logicalnot",
    "mu",
    "trademark",
    "Eth",
    "onehalf",
    "plusminus",
    "Thorn",
    "onequarter",
    "divide",
    "brokenbar",
    "degree",
    "thorn",
    "threequarters",
    "twosuperior",
    "registered",
    "minus",
    "eth",
    "multiply",
    "threesuperior",
    "copyright",
    "Aacute",
    "Acircumflex",
    "Adieresis",
    "Agrave",
    "Aring",
    "Atilde",
    "Ccedilla",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Ntilde",
    "Oacute",
    "Ocircumflex",
    "Odieresis",
    "Ograve",
    "Otilde",
    "Scaron",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Ugrave",
    "Yacute",
    "Ydieresis",
    "Zcaron",
    "aacute",
    "acircumflex",
    "adieresis",
    "agrave",
    "aring",
    "atilde",
    "ccedilla",
    "eacute",
    "ecircumflex",
    "edieresis",
    "egrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "igrave",
    "ntilde",
    "oacute",
    "ocircumflex",
    "odieresis",
    "ograve",
    "otilde",
    "scaron",
    "uacute",
    "ucircumflex",
    "udieresis",
    "ugrave",
    "yacute",
    "ydieresis",
    "zcaron",
];

/// The predefined string of a standard SID.
pub(super) fn standard_string(sid: u16) -> Option<&'static str> {
    let sid = usize::from(sid);
    ISO_ADOBE
        .get(sid)
        .or_else(|| predefined::EXPERT_STRINGS.get(sid.checked_sub(ISO_ADOBE.len())?))
        .copied()
}

/// The StandardEncoding glyph name for `code` (TN 5176 Appendix B), which
/// Type 1 fonts share.
pub(crate) fn standard(code: u8) -> Option<Name> {
    standard_sid(code)
        .and_then(standard_string)
        .map(|name| Name::new(name.as_bytes()))
}

/// StandardEncoding as a code-to-name table.
pub(crate) fn standard_encoding() -> Box<[Option<Name>; 256]> {
    Box::new(std::array::from_fn(|code| {
        u8::try_from(code).ok().and_then(standard)
    }))
}

fn standard_sid(code: u8) -> Option<u16> {
    Some(match code {
        32..=126 => u16::from(code) - 31,
        161..=175 => u16::from(code) - 65,
        177..=180 => u16::from(code) - 66,
        182..=189 => u16::from(code) - 67,
        191 => 123,
        193..=200 => u16::from(code) - 69,
        202 => 132,
        203 => 133,
        205..=208 => u16::from(code) - 71,
        225 => 138,
        227 => 139,
        232 => 140,
        233 => 141,
        234 => 142,
        235 => 143,
        241 => 144,
        245 => 145,
        248 => 146,
        249 => 147,
        250 => 148,
        251 => 149,
        _ => return None,
    })
}

/// The string for `sid`: a standard string, or one from the String INDEX
/// `strings` of the program `data`. `at` locates the reference.
pub(super) fn sid(
    sid: u16,
    strings: &Index,
    data: &[u8],
    work: &Work<'_>,
    at: Position,
) -> Result<Name> {
    if sid >= RESERVED_SID {
        return Err(syntax(at, "reserved CFF SID"));
    }
    if let Some(value) = standard_string(sid) {
        work.string(value.len(), at)?;
        return Ok(Name::new(value.as_bytes()));
    }
    let index = usize::from(sid) - STANDARD_STRINGS;
    let range = strings
        .range(index)
        .ok_or_else(|| syntax(at, "CFF SID outside String INDEX"))?;
    let start = Position::Buffer(range.start);
    work.string(range.len(), start)?;
    let bytes = strings
        .get(data, index)
        .ok_or_else(|| syntax(start, "CFF string outside font"))?
        .take_rest();
    Name::try_new(bytes).map_err(|e| e.at(start))
}

/// The built-in encoding at Top DICT `offset` of the CFF `table`
/// (TN 5176 section 12).
pub(super) fn encoding(
    table: &Cursor<'_>,
    offset: usize,
    charset: &[u16],
    strings: &Index,
    data: &[u8],
    work: &mut Work<'_>,
) -> Result<Box<[Option<Name>; 256]>> {
    if offset == STANDARD_ENCODING {
        return Ok(standard_encoding());
    }
    let mut result = Box::new(std::array::from_fn(|_| None));
    if offset == EXPERT_ENCODING {
        for code in 0..=255_u8 {
            if let Some(id) = predefined::expert_sid(code) {
                result[usize::from(code)] = Some(sid(id, strings, data, work, table.position())?);
            }
        }
        return Ok(result);
    }
    let mut r = table.cursor_at(offset)?;
    let format = r.u8()?;
    let count = r.u8()?;
    let ranges = match format & FORMAT_MASK {
        CODES => false,
        RANGES => true,
        _ => return Err(r.error("invalid CFF encoding format")),
    };
    let mut gid = 1;
    for _ in 0..count {
        let first = r.u8()?;
        let left = if ranges { r.u8()? } else { 0 };
        let end = first
            .checked_add(left)
            .ok_or_else(|| limit(r.position(), "CFF encoding code overflow"))?;
        for code in first..=end {
            let id = *charset
                .get(gid)
                .ok_or_else(|| r.error("CFF encoding exceeds charset"))?;
            let slot = &mut result[usize::from(code)];
            if slot.is_some() {
                return Err(r.error("duplicate CFF encoding code"));
            }
            *slot = Some(sid(id, strings, data, work, r.position())?);
            gid += 1;
        }
    }
    if format & HAS_SUPPLEMENTS != 0 {
        let encoded = charset.get(1..gid).unwrap_or_default();
        supplements::read(&mut r, &mut result, encoded, strings, data, work)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{STANDARD_STRINGS, standard, standard_string};

    #[test]
    fn standard_strings_are_indexed_by_sid() {
        assert_eq!(standard_string(0), Some(".notdef"));
        assert_eq!(standard_string(228), Some("zcaron"));
        assert_eq!(standard_string(229), Some("exclamsmall"));
        assert_eq!(standard_string(390), Some("Semibold"));
        assert_eq!(
            standard_string(u16::try_from(STANDARD_STRINGS).unwrap()),
            None
        );
        assert_eq!(standard(b'A').unwrap().as_bytes(), b"A");
        assert_eq!(standard(0xe1).unwrap().as_bytes(), b"AE");
        assert_eq!(standard(0xfb).unwrap().as_bytes(), b"germandbls");
        assert_eq!(standard(0x80), None);
    }
}
