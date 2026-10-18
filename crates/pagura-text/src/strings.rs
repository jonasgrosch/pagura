//! PDF text strings (7.9.2.2), used by ActualText and Alt, not font codes.
use pagura_core::{Error, Limits, Result};

/// Decodes a PDF text string: BOM-marked UTF-16BE/UTF-8, otherwise PDFDocEncoding.
/// Language escape metadata is removed from Unicode strings (7.9.2.2.2).
///
/// # Errors
/// Invalid Unicode, undefined PDFDocEncoding bytes and exceeded limits fail
/// explicitly; no replacement text is invented.
pub fn decode_text_string(bytes: &[u8], limits: Limits) -> Result<String> {
    if bytes.len() > limits.operations {
        return Err(Error::limit("PDF text string work budget exceeded"));
    }
    if bytes.len().saturating_mul(4) > limits.string_bytes {
        return Err(Error::limit("PDF text string byte budget exceeded"));
    }
    let (text, unicode) = if let Some(data) = bytes.strip_prefix(&[0xfe, 0xff]) {
        if data.len() % 2 != 0 {
            return Err(Error::syntax("odd UTF-16 text string length"));
        }
        let text = char::decode_utf16(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_be_bytes([p[0], p[1]])),
        )
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| Error::syntax("invalid UTF-16 text string"))?;
        (text, true)
    } else if let Some(data) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (
            std::str::from_utf8(data)
                .map_err(|_| Error::syntax("invalid UTF-8 text string"))?
                .to_owned(),
            true,
        )
    } else {
        let table = pdf_doc();
        let mut text = String::new();
        for &byte in bytes {
            let value = table[usize::from(byte)]
                .as_deref()
                .ok_or(Error::syntax("undefined PDFDocEncoding text string byte"))?;
            text.push_str(value);
        }
        (text, false)
    };
    if !unicode || !text.contains('\u{1b}') {
        return Ok(text);
    }
    let mut output = String::new();
    let mut metadata = false;
    for c in text.chars() {
        if c == '\u{1b}' {
            metadata = !metadata;
        } else if !metadata {
            output.push(c);
        }
    }
    if metadata {
        return Err(Error::syntax("unterminated text string language escape"));
    }
    Ok(output)
}

fn pdf_doc() -> &'static [Option<String>; 256] {
    static TABLE: std::sync::OnceLock<[Option<String>; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = std::array::from_fn(|_| None);
        for &(code, name) in crate::tables::PDF_DOC {
            table[usize::from(code)] = crate::encoding::unicode(name.as_bytes());
        }
        table
    })
}
