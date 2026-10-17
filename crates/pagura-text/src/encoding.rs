//! Simple encodings and the glyph-name algorithm (9.6.5, 9.10.2, Annex D).
use crate::tables;
use pagura_core::{Error, Limits, Name, Result, Value};
use pagura_document::Document;

pub(crate) type Encoding = [Option<Name>; 256];

#[derive(Debug)]
pub(crate) struct SimpleMapping {
    pub(crate) names: Encoding,
    pub(crate) unicode: [Option<String>; 256],
}

impl SimpleMapping {
    pub(crate) fn new(names: Encoding, limits: Limits) -> Result<Self> {
        let mut work = 0;
        let mut bytes = 0;
        let mut texts = std::array::from_fn(|_| None);
        for (slot, name) in texts.iter_mut().zip(&names) {
            if let Some(name) = name {
                crate::cmap::charge(&mut work, name.len(), limits.operations)?;
                *slot = unicode(name.as_bytes());
                crate::cmap::charge(
                    &mut bytes,
                    slot.as_ref().map_or(0, String::len),
                    limits.decoded_bytes,
                )?;
            }
        }
        Ok(Self {
            names,
            unicode: texts,
        })
    }
}

pub(crate) fn base(name: &[u8]) -> Result<Encoding> {
    let table = match name {
        b"StandardEncoding" => tables::STANDARD,
        b"MacRomanEncoding" => tables::MAC_ROMAN,
        b"WinAnsiEncoding" => tables::WIN_ANSI,
        b"MacExpertEncoding" => tables::MAC_EXPERT,
        b"PDFDocEncoding" => tables::PDF_DOC,
        b"Symbol" => tables::SYMBOL,
        b"ZapfDingbats" => tables::ZAPF_DINGBATS,
        _ => return Err(Error::syntax("unknown simple font encoding")),
    };
    let mut encoding = std::array::from_fn(|_| None);
    for &(code, name) in table {
        encoding[usize::from(code)] = Some(Name::new(name.as_bytes()));
    }
    Ok(encoding)
}

fn predefined(name: &[u8]) -> Result<Encoding> {
    match name {
        b"MacRomanEncoding" | b"MacExpertEncoding" | b"WinAnsiEncoding" => base(name),
        _ => Err(Error::syntax("invalid PDF predefined simple encoding")),
    }
}

pub(crate) fn load(
    document: &Document,
    value: Option<&Value>,
    default: Encoding,
) -> Result<Encoding> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = document.resolve(value)?;
    if let Some(name) = value.as_name() {
        return predefined(name.as_bytes());
    }
    let dict = value
        .as_dictionary()
        .ok_or_else(|| Error::syntax("invalid font Encoding"))?;
    let mut encoding = if let Some(value) = dict.get(b"BaseEncoding") {
        let name = document
            .resolve(value)?
            .as_name()
            .ok_or_else(|| Error::syntax("invalid BaseEncoding"))?;
        predefined(name.as_bytes())?
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
                return Err(Error::syntax("invalid Differences element"));
            }
        }
    }
    Ok(encoding)
}

/// AGL name decomposition: suffix removal, components, uniXXXX and uXXXX.
/// Unknown names remain unknown rather than inventing a Unicode value.
pub(crate) fn unicode(name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?.split('.').next()?;
    let mut result = String::new();
    for component in name.split('_') {
        if let Ok(index) = tables::UNICODE.binary_search_by_key(&component, |(name, _)| *name) {
            result.push(char::from_u32(tables::UNICODE[index].1)?);
        } else if let Some(hex) = component.strip_prefix("uni") {
            if hex.is_empty() || hex.len() % 4 != 0 || !upper_hex(hex) {
                return None;
            }
            for chunk in hex.as_bytes().as_chunks::<4>().0 {
                let value = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
                result.push(char::from_u32(value)?);
            }
        } else {
            let hex = component.strip_prefix('u')?;
            if !(4..=6).contains(&hex.len()) || !upper_hex(hex) {
                return None;
            }
            result.push(char::from_u32(u32::from_str_radix(hex, 16).ok()?)?);
        }
    }
    (!result.is_empty()).then_some(result)
}

fn upper_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_encoding_exceptions() -> Result<()> {
        assert_eq!(unicode(b"uni00410042_u1F600.alt").as_deref(), Some("AB😀"));
        for name in [
            b"uniD800".as_slice(),
            b"u110000",
            b"uni00e9",
            b"unknown",
            b"A__B",
        ] {
            assert_eq!(unicode(name), None);
        }
        let win = base(b"WinAnsiEncoding")?;
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
        let mac = base(b"MacRomanEncoding")?;
        assert_eq!(
            mac[219].as_ref().map(Name::as_bytes),
            Some(b"currency".as_slice())
        );
        Ok(())
    }
}
