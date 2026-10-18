//! PDF text strings (ISO 32000-2 clause 7.9.2.2) and PDFDocEncoding
//! (Annex D.3): the one decoder for outline titles, annotation contents,
//! `/ActualText`, document information and every other text string, and the
//! one encoder for legacy passwords and written text.
//!
//! A text string is UTF-16BE when it starts with the byte order mark
//! `FE FF`, UTF-8 when it starts with `EF BB BF` (PDF 2.0), and
//! PDFDocEncoding otherwise. Unicode strings may contain language escape
//! sequences (7.9.2.2.2), which carry metadata and are not text.

use crate::repairs::{INVALID_TEXT, LANGUAGE_ESCAPE};
use crate::{Diagnostic, Diagnostics, Error, Limits, Result};

const UTF16_BOM: [u8; 2] = [0xfe, 0xff];
const UTF8_BOM: [u8; 3] = [0xef, 0xbb, 0xbf];
const ESCAPE: u8 = 0x1b;

/// PDFDocEncoding 0x18–0x1F: spacing accents (Annex D.2).
const ACCENTS: [char; 8] = [
    '\u{02d8}', '\u{02c7}', '\u{02c6}', '\u{02d9}', '\u{02dd}', '\u{02db}', '\u{02da}', '\u{02dc}',
];

/// PDFDocEncoding 0x80–0x9E: punctuation, ligatures and Latin letters
/// (Annex D.2).
const HIGH: [char; 31] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203a}', '\u{2212}', '\u{2030}', '\u{201e}', '\u{201c}', '\u{201d}', '\u{2018}',
    '\u{2019}', '\u{201a}', '\u{2122}', '\u{fb01}', '\u{fb02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017d}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017e}',
];

/// The character a PDFDocEncoding byte stands for, or `None` for the
/// undefined codes 0x00–0x08, 0x0B, 0x0C, 0x0E–0x17, 0x7F, 0x9F and 0xAD
/// (Annex D.2).
pub fn pdf_doc_char(byte: u8) -> Option<char> {
    match byte {
        0x09 | 0x0a | 0x0d | 0x20..=0x7e | 0xa1..=0xac | 0xae..=0xff => Some(char::from(byte)),
        0x18..=0x1f => ACCENTS.get(usize::from(byte - 0x18)).copied(),
        0x80..=0x9e => HIGH.get(usize::from(byte - 0x80)).copied(),
        0xa0 => Some('\u{20ac}'),
        _ => None,
    }
}

/// The PDFDocEncoding byte for `character`, or `None` if Annex D.2 does not
/// define one.
pub fn pdf_doc_byte(character: char) -> Option<u8> {
    match u32::from(character) {
        code @ (0x09 | 0x0a | 0x0d | 0x20..=0x7e | 0xa1..=0xac | 0xae..=0xff) => {
            u8::try_from(code).ok()
        }
        0x20ac => Some(0xa0),
        _ => [(0x18_u8, ACCENTS.as_slice()), (0x80, HIGH.as_slice())]
            .into_iter()
            .find_map(|(first, table)| {
                let index = table.iter().position(|&c| c == character)?;
                first.checked_add(u8::try_from(index).ok()?)
            }),
    }
}

/// Encodes `text` in PDFDocEncoding, or returns `None` if a character has no
/// PDFDocEncoding byte. Nothing is replaced or dropped. This is the password
/// encoding of the legacy security handlers (7.6.4.3.2, Algorithm 2 step a).
pub fn encode_pdf_doc(text: &str) -> Option<Vec<u8>> {
    text.chars().map(pdf_doc_byte).collect()
}

/// Encodes `text` as a text string: PDFDocEncoding when every character has
/// a byte there and the result cannot be mistaken for a byte order mark,
/// otherwise UTF-16BE with a byte order mark. [`decode`] returns `text`.
pub fn encode(text: &str) -> Vec<u8> {
    match encode_pdf_doc(text) {
        Some(bytes) if !bytes.starts_with(&UTF16_BOM) && !bytes.starts_with(&UTF8_BOM) => bytes,
        _ => UTF16_BOM
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
    }
}

/// Decodes a text string, removing language escape sequences.
///
/// Invalid UTF-16 or UTF-8, a byte PDFDocEncoding does not define, and a
/// malformed language escape are deviations. They go through `diagnostics`:
/// in strict mode they are errors; in repair mode an invalid sequence
/// becomes U+FFFD and a malformed escape character is dropped, each kind
/// reported once per string. The diagnostics and errors carry no position:
/// the caller adds the string's object and position.
///
/// # Errors
///
/// A `Limit` error if the input or the decoded text exceeds
/// [`Limits::string_bytes`]; under [`Recovery::Strict`](crate::Recovery) a
/// syntax error tagged [`INVALID_TEXT`] or [`LANGUAGE_ESCAPE`].
pub fn decode(bytes: &[u8], limits: &Limits, diagnostics: &mut Diagnostics) -> Result<String> {
    if bytes.len() > limits.string_bytes {
        return Err(Error::limit("text string budget exceeded"));
    }
    let mut text = Text {
        output: String::new(),
        max: limits.string_bytes,
        diagnostics,
        invalid: false,
        escape: false,
    };
    if let Some(data) = bytes.strip_prefix(&UTF16_BOM) {
        text.utf16(data)?;
    } else if let Some(data) = bytes.strip_prefix(&UTF8_BOM) {
        text.utf8(data)?;
    } else {
        for &byte in bytes {
            match pdf_doc_char(byte) {
                Some(character) => text.push(character)?,
                None => text.invalid()?,
            }
        }
    }
    Ok(text.output)
}

/// Decoded output with its budget and per-string reporting state.
struct Text<'a> {
    output: String,
    max: usize,
    diagnostics: &'a mut Diagnostics,
    invalid: bool,
    escape: bool,
}

impl Text<'_> {
    fn push_str(&mut self, text: &str) -> Result<()> {
        self.output
            .len()
            .checked_add(text.len())
            .filter(|&length| length <= self.max)
            .ok_or(Error::limit("text string budget exceeded"))?;
        self.output
            .try_reserve(text.len())
            .map_err(|_| Error::limit("text string allocation failed"))?;
        self.output.push_str(text);
        Ok(())
    }

    fn push(&mut self, character: char) -> Result<()> {
        self.push_str(character.encode_utf8(&mut [0; 4]))
    }

    /// An invalid sequence: rejected, or replaced by U+FFFD.
    fn invalid(&mut self) -> Result<()> {
        if !self.invalid {
            self.diagnostics.repair(Diagnostic::new(
                INVALID_TEXT,
                "invalid text string sequence replaced (7.9.2.2, Annex D.3)",
            ))?;
            self.invalid = true;
        }
        self.push(char::REPLACEMENT_CHARACTER)
    }

    /// An escape character that starts no language escape: rejected, or
    /// dropped.
    fn malformed_escape(&mut self) -> Result<()> {
        if !self.escape {
            self.diagnostics.repair(Diagnostic::new(
                LANGUAGE_ESCAPE,
                "malformed text string language escape removed (7.9.2.2.2)",
            ))?;
            self.escape = true;
        }
        Ok(())
    }

    fn utf16(&mut self, data: &[u8]) -> Result<()> {
        let (units, odd) = data.as_chunks::<2>();
        let mut index = 0;
        while let Some(&unit) = units.get(index) {
            let first = u16::from_be_bytes(unit);
            if first == u16::from(ESCAPE) {
                if let Some(length) = utf16_language(units.get(index + 1..).unwrap_or_default()) {
                    index += length + 2;
                } else {
                    self.malformed_escape()?;
                    index += 1;
                }
                continue;
            }
            let second = units
                .get(index + 1)
                .map_or(0, |&unit| u16::from_be_bytes(unit));
            if let Some(Ok(character)) = char::decode_utf16([first, second]).next() {
                self.push(character)?;
                index += character.len_utf16();
            } else {
                self.invalid()?;
                index += 1;
            }
        }
        if odd.is_empty() {
            Ok(())
        } else {
            self.invalid()
        }
    }

    fn utf8(&mut self, data: &[u8]) -> Result<()> {
        let mut rest = data;
        loop {
            let split = rest
                .iter()
                .position(|&byte| byte == ESCAPE)
                .unwrap_or(rest.len());
            let (segment, tail) = rest.split_at(split);
            for chunk in segment.utf8_chunks() {
                self.push_str(chunk.valid())?;
                if !chunk.invalid().is_empty() {
                    self.invalid()?;
                }
            }
            let Some((_, after)) = tail.split_first() else {
                return Ok(());
            };
            rest = if let Some(length) = utf8_language(after) {
                after.get(length + 1..).unwrap_or_default()
            } else {
                self.malformed_escape()?;
                after
            };
        }
    }
}

/// The length in code units of a UTF-16 language escape's codes, given the
/// units after its opening escape: one unit holding a two-letter ISO 639
/// language code, optionally a second holding an ISO 3166 country code, then
/// the closing escape (7.9.2.2.2).
fn utf16_language(units: &[[u8; 2]]) -> Option<usize> {
    [1, 2].into_iter().find(|&length| {
        units.get(length) == Some(&[0, ESCAPE])
            && units
                .get(..length)
                .is_some_and(|codes| codes.as_flattened().iter().all(u8::is_ascii_alphabetic))
    })
}

/// The length in bytes of a UTF-8 language escape's codes, given the bytes
/// after its opening escape: two or four ASCII letters, then the closing
/// escape (7.9.2.2.2).
fn utf8_language(bytes: &[u8]) -> Option<usize> {
    [2, 4].into_iter().find(|&length| {
        bytes.get(length) == Some(&ESCAPE)
            && bytes
                .get(..length)
                .is_some_and(|codes| codes.iter().all(u8::is_ascii_alphabetic))
    })
}

#[cfg(test)]
mod tests;
