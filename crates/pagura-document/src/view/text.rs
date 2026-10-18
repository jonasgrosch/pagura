//! Text strings (7.9.2.2, Annex D.3), decoded independently of page fonts.
use crate::tags;
use crate::view::{Context, Source};
use pagura_core::{Error, Result};

pub(crate) fn decode(ctx: &mut Context<'_>, bytes: &[u8], source: Source) -> Result<String> {
    let text = if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        collect(ctx, source, || {
            bytes.utf8_chunks().flat_map(|chunk| {
                chunk
                    .valid()
                    .chars()
                    .map(Some)
                    .chain(std::iter::once(None).take(usize::from(!chunk.invalid().is_empty())))
            })
        })?
    } else if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        collect(ctx, source, || {
            char::decode_utf16(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_be_bytes(*b)),
            )
            .map(Result::ok)
            .chain(std::iter::once(None).take(bytes.len() % 2))
        })?
    } else {
        collect(ctx, source, || bytes.iter().copied().map(pdf_char))?
    };
    language_escapes(text, source)
}
fn collect<I: Iterator<Item = Option<char>>>(
    ctx: &mut Context<'_>,
    source: Source,
    decode: impl Fn() -> I,
) -> Result<String> {
    let mut length = 0usize;
    let mut invalid = false;
    for c in decode() {
        invalid |= c.is_none();
        length = length
            .checked_add(c.unwrap_or('\u{fffd}').len_utf8())
            .filter(|&n| n <= ctx.doc.limits().string_bytes)
            .ok_or_else(|| source.error(Error::limit("navigation text budget exceeded")))?;
    }
    if invalid {
        ctx.repair(
            tags::NAVIGATION_TEXT,
            "invalid navigation Unicode or undefined PDFDocEncoding byte replaced (7.9.2.2, D.3)",
            source,
        )?;
    }
    ctx.allocate::<u8>(length, source)?;
    let mut text = String::with_capacity(length);
    text.extend(decode().map(|c| c.unwrap_or('\u{fffd}')));
    Ok(text)
}
fn pdf_char(b: u8) -> Option<char> {
    Some(match b {
        9 | 10 | 13 => char::from(b),
        0..=23 | 127 | 159 | 173 => return None,
        24..=31 => [
            '\u{02d8}', '\u{02c7}', '\u{02c6}', '\u{02d9}', '\u{02dd}', '\u{02db}', '\u{02da}',
            '\u{02dc}',
        ][usize::from(b - 24)],
        128..=158 => [
            '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’',
            '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž',
        ][usize::from(b - 128)],
        160 => '€',
        _ => char::from(b),
    })
}
/// Language runs annotate Unicode strings; they are not visible title text
/// (7.9.2.2.2). Compact in place, preserving UTF-8 boundaries and other text.
fn language_escapes(text: String, source: Source) -> Result<String> {
    let mut bytes = text.into_bytes();
    let mut read = 0;
    let mut write = 0;
    while read < bytes.len() {
        if bytes[read] == 27 {
            let tail = &bytes[read + 1..];
            let length = [2, 4].into_iter().find(|&n| {
                tail.get(n) == Some(&27) && tail[..n].iter().all(u8::is_ascii_alphabetic)
            });
            if let Some(length) = length {
                read += length + 2;
                continue;
            }
        }
        bytes[write] = bytes[read];
        write += 1;
        read += 1;
    }
    bytes.truncate(write);
    String::from_utf8(bytes).map_err(|_| source.error(Error::syntax("invalid navigation Unicode")))
}
