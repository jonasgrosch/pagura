//! Bounded, reported CMap interpretations; strict mode rejects each deviation.
use super::{CMap, Code, ParseContext, Range, charge};
use crate::repairs;
use pagura_core::{Error, Position, Result, Value};
use std::collections::{BTreeMap, BTreeSet};

impl CMap {
    pub(super) fn codespace(
        &mut self,
        range: Range,
        context: &mut ParseContext<'_>,
        position: Position,
    ) -> Result<()> {
        let rectangular = (0..usize::from(range.first.len)).all(|i| {
            let shift = i * 8;
            ((range.first.value >> shift) & 255) <= ((range.last.value >> shift) & 255)
        });
        if rectangular {
            return self.push_space(range, context);
        }
        context.report.repair(
            repairs::CMAP_CODESPACE,
            "codespace uses an integer interval instead of byte bounds (9.7.6.2)",
            position,
        )?;
        // A numeric interval has a unique decomposition into aligned binary
        // blocks. Each is a valid byte rectangle; no code is added or lost.
        let mut cursor = u64::from(range.first.value);
        let last = u64::from(range.last.value);
        while cursor <= last {
            charge(&mut context.work, 1, context.limits.operations)?;
            let remaining = last - cursor + 1;
            let bits = cursor.trailing_zeros().min(remaining.ilog2()).min(32);
            let size = 1_u64 << bits;
            let first =
                u32::try_from(cursor).map_err(|_| Error::syntax("invalid codespace start"))?;
            let end = u32::try_from(cursor + size - 1)
                .map_err(|_| Error::syntax("invalid codespace end"))?;
            self.push_space(
                Range {
                    first: Code {
                        value: first,
                        len: range.first.len,
                    },
                    last: Code {
                        value: end,
                        len: range.first.len,
                    },
                },
                context,
            )?;
            cursor += size;
        }
        Ok(())
    }

    fn push_space(&mut self, range: Range, context: &ParseContext<'_>) -> Result<()> {
        if self.spaces.len() >= context.limits.value_nodes {
            return Err(Error::limit("CMap codespace limit exceeded"));
        }
        self.spaces.push(range);
        Ok(())
    }

    pub(super) fn unicode_range(
        &mut self,
        range: Range,
        value: &Value,
        context: &mut ParseContext<'_>,
        position: Position,
    ) -> Result<()> {
        let count = u64::from(range.last.value) - u64::from(range.first.value) + 1;
        let count = usize::try_from(count).map_err(|_| Error::limit("Unicode range too large"))?;
        charge(&mut context.work, count, context.limits.operations)?;
        if count
            > context
                .limits
                .value_nodes
                .saturating_sub(self.unicode.len())
        {
            return Err(Error::limit("Unicode mapping limit exceeded"));
        }
        if let Some(array) = value.as_array() {
            if array.len() != count {
                return Err(Error::syntax("bfrange array length mismatch"));
            }
            for (code, value) in (range.first.value..=range.last.value).zip(array) {
                let bytes = value
                    .as_string()
                    .ok_or_else(|| Error::syntax("invalid Unicode destination"))?;
                self.insert_text(
                    Code {
                        value: code,
                        len: range.first.len,
                    },
                    bytes,
                    context,
                    position,
                )?;
            }
        } else {
            let mut bytes = value
                .as_string()
                .ok_or_else(|| Error::syntax("invalid Unicode destination"))?
                .to_vec();
            if bytes.is_empty() && count > 1 {
                return Err(Error::syntax("empty bfrange destination cannot increment"));
            }
            let overflow = bytes
                .last()
                .is_some_and(|last| count - 1 > usize::from(255 - *last));
            if overflow {
                context.report.repair(
                    repairs::TOUNICODE_RANGE,
                    "bfrange destination overflows its final byte (9.10.3)",
                    position,
                )?;
            }
            for code in range.first.value..=range.last.value {
                self.insert_text(
                    Code {
                        value: code,
                        len: range.first.len,
                    },
                    &bytes,
                    context,
                    position,
                )?;
                if code != range.last.value && !increment(&mut bytes) {
                    break;
                }
            }
        }
        Ok(())
    }

    fn insert_text(
        &mut self,
        code: Code,
        bytes: &[u8],
        context: &mut ParseContext<'_>,
        position: Position,
    ) -> Result<()> {
        if bytes.len() > 512 {
            return Err(Error::syntax("Unicode destination exceeds 512 bytes"));
        }
        charge(&mut context.work, bytes.len(), context.limits.operations)?;
        let text = if bytes.len().is_multiple_of(2) {
            char::decode_utf16(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_be_bytes(*b)),
            )
            .collect::<std::result::Result<String, _>>()
            .ok()
        } else {
            None
        };
        let Some(text) = text else {
            context.report.repair(
                repairs::TOUNICODE_UTF16,
                "invalid UTF-16BE destination omitted (9.10.3)",
                position,
            )?;
            let old = self.unicode.insert(code, None).flatten();
            self.unicode_bytes = self
                .unicode_bytes
                .saturating_sub(old.as_ref().map_or(0, String::len));
            return Ok(());
        };
        let old = self
            .unicode
            .get(&code)
            .and_then(Option::as_ref)
            .map_or(0, String::len);
        self.unicode_bytes = self.unicode_bytes.saturating_sub(old);
        charge(
            &mut self.unicode_bytes,
            text.len(),
            context.limits.decoded_bytes,
        )?;
        self.unicode.insert(code, Some(text));
        Ok(())
    }

    pub(crate) fn simple_compatible(
        &mut self,
        document: &pagura_document::Document,
        position: Position,
    ) -> Result<()> {
        if self.simple_codes() {
            return Ok(());
        }
        if self.spaces.iter().any(|space| space.first.len > 2)
            || self.unicode.keys().any(|code| code.len > 2)
        {
            return Err(Error::syntax(
                "unsupported simple-font ToUnicode code length",
            ));
        }
        repairs::Report::new(document).repair(
            repairs::TOUNICODE_SIMPLE_CODES,
            "simple-font ToUnicode uses two-byte source codes (9.10.3)",
            position,
        )?;
        let mut unicode = BTreeMap::new();
        let mut ambiguous = BTreeSet::new();
        let mut work = 0;
        for (code, text) in std::mem::take(&mut self.unicode) {
            charge(&mut work, 1, document.limits().operations)?;
            let code = Code {
                value: code.value & 255,
                len: 1,
            };
            if ambiguous.contains(&code) {
                continue;
            }
            if unicode.get(&code).is_some_and(|old| *old != text) {
                unicode.remove(&code);
                ambiguous.insert(code);
            } else {
                unicode.insert(code, text);
            }
        }
        self.unicode = unicode;
        self.unicode_bytes = self
            .unicode
            .values()
            .filter_map(Option::as_ref)
            .map(String::len)
            .sum();
        self.spaces = vec![Range {
            first: Code { value: 0, len: 1 },
            last: Code { value: 255, len: 1 },
        }];
        Ok(())
    }
}

// Use big-endian byte carry only after reporting the out-of-spec range. A full
// string overflow terminates the range; wrapped destinations are never invented.
fn increment(bytes: &mut [u8]) -> bool {
    for byte in bytes.iter_mut().rev() {
        if *byte < 255 {
            *byte += 1;
            return true;
        }
        *byte = 0;
    }
    false
}
