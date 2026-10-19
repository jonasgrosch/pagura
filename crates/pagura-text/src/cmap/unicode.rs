//! ToUnicode destinations as disjoint code intervals (9.10.3). A `bfrange`
//! is stored once and its text is computed on lookup, so the map grows with
//! the CMap's size, never with the number of codes a range covers.
use super::{CMap, Code, ParseContext, Range};
use crate::{GlyphText, repairs};
use pagura_core::{Error, Meter, Position, Result, Value};
use std::collections::BTreeMap;

/// The longest UTF-16BE destination string a ToUnicode CMap may use (9.10.3).
const MAX_DESTINATION: usize = 512;

#[derive(Debug, Clone, Default)]
pub(super) struct UnicodeMap(BTreeMap<Code, Entry>);

#[derive(Debug, Clone)]
struct Entry {
    last: Code,
    text: Text,
}

#[derive(Debug, Clone, PartialEq)]
enum Text {
    /// The destination was invalid UTF-16BE: the code has no text, and an
    /// inherited mapping does not apply either.
    Absent,
    /// One destination for every code of the entry.
    Fixed(GlyphText),
    /// A `bfrange` string destination: the UTF-16BE text of the entry's first
    /// code; each later code adds its offset to the last byte, with carry.
    Increment(Box<[u8]>),
}

/// The text a map assigns to a code.
pub(super) enum Lookup {
    Text(GlyphText),
    /// Mapped to no text; inherited mappings are shadowed.
    Absent,
    /// Not mapped here.
    Missing,
}

impl UnicodeMap {
    pub(super) fn get(&self, code: Code) -> Lookup {
        let Some((first, entry)) = self.0.range(..=code).next_back() else {
            return Lookup::Missing;
        };
        if first.len != code.len || code.value > entry.last.value {
            return Lookup::Missing;
        }
        match &entry.text {
            Text::Absent => Lookup::Absent,
            Text::Fixed(text) => Lookup::Text(text.clone()),
            Text::Increment(base) => {
                let mut buffer = [0; MAX_DESTINATION];
                let bytes = &mut buffer[..base.len()];
                bytes.copy_from_slice(base);
                if !add(bytes, code.value - first.value) {
                    return Lookup::Absent;
                }
                utf16(bytes).map_or(Lookup::Absent, Lookup::Text)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn codes(&self) -> impl Iterator<Item = (Code, Code)> + '_ {
        self.0.iter().map(|(first, entry)| (*first, entry.last))
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.0.values().fold(
            crate::memory::tree::<Code, Entry>(self.0.len()),
            |sum, entry| {
                sum.saturating_add(match &entry.text {
                    Text::Absent => 0,
                    Text::Fixed(text) => text.heap_bytes(),
                    Text::Increment(base) => base.len(),
                })
            },
        )
    }

    /// Maps `first..=last` (one code length), overriding earlier entries.
    /// Text bytes of new entries are charged to `text`; entries that the new
    /// one replaces entirely are refunded.
    fn insert(
        &mut self,
        first: Code,
        last: Code,
        text: Text,
        work: &mut Meter,
        bytes: &mut Meter,
    ) -> Result<()> {
        let mut overlap = Vec::new();
        if let Some((&key, old)) = self.0.range(..first).next_back()
            && key.len == first.len
            && old.last.value >= first.value
        {
            overlap.push(key);
        }
        for (&key, _) in self.0.range(first..=last) {
            work.charge(1)?;
            overlap.push(key);
        }
        for key in overlap {
            work.charge(1)?;
            let Some(old) = self.0.remove(&key) else {
                continue;
            };
            let mut kept = false;
            if key.value < first.value {
                let head = Code {
                    value: first.value - 1,
                    len: key.len,
                };
                self.0.insert(
                    key,
                    Entry {
                        last: head,
                        text: old.text.clone(),
                    },
                );
                kept = true;
            }
            if old.last.value > last.value {
                let start = Code {
                    value: last.value + 1,
                    len: key.len,
                };
                let tail = match &old.text {
                    Text::Increment(base) => {
                        let mut base = base.clone();
                        if add(&mut base, start.value - key.value) {
                            bytes.charge(base.len())?;
                            Text::Increment(base)
                        } else {
                            Text::Absent
                        }
                    }
                    other => other.clone(),
                };
                self.0.insert(
                    start,
                    Entry {
                        last: old.last,
                        text: tail,
                    },
                );
                kept = true;
            }
            if !kept {
                bytes.refund(text_bytes(&old.text));
            }
        }
        bytes.charge(text_bytes(&text))?;
        self.0.insert(first, Entry { last, text });
        Ok(())
    }

    /// Adds an entry produced by flattening; used by the simple-font repair.
    pub(super) fn insert_flat(&mut self, code: Code, text: Option<GlyphText>) {
        let text = text.map_or(Text::Absent, Text::Fixed);
        self.0.insert(code, Entry { last: code, text });
    }
}

fn text_bytes(text: &Text) -> usize {
    match text {
        Text::Absent => 0,
        Text::Fixed(text) => text.len(),
        Text::Increment(base) => base.len(),
    }
}

impl CMap {
    /// A `bfchar` or `bfrange` entry (9.10.3). Every code of a range is
    /// validated and charged as work, but the map stores one entry per run of
    /// valid (or invalid) destinations.
    pub(super) fn unicode_range(
        &mut self,
        range: Range,
        value: &Value,
        context: &mut ParseContext<'_, '_>,
        position: Position,
    ) -> Result<()> {
        let count = u64::from(range.last.value) - u64::from(range.first.value) + 1;
        let count = usize::try_from(count).map_err(|_| Error::limit("Unicode range too large"))?;
        context.budget.work.charge(count)?;
        if let Some(array) = value.as_array() {
            if array.len() != count {
                return Err(Error::syntax("bfrange array length mismatch"));
            }
            for (code, value) in (range.first.value..=range.last.value).zip(array) {
                let code = Code {
                    value: code,
                    len: range.first.len,
                };
                let bytes = value
                    .as_string()
                    .ok_or(Error::syntax("invalid Unicode destination"))?;
                let text = Self::destination(bytes, context, position)?;
                self.insert_unicode(code, code, text, context)?;
            }
            return Ok(());
        }
        let mut bytes = value
            .as_string()
            .ok_or(Error::syntax("invalid Unicode destination"))?
            .to_vec();
        if bytes.is_empty() && count > 1 {
            return Err(Error::syntax("empty bfrange destination cannot increment"));
        }
        if bytes.len() > MAX_DESTINATION {
            return Err(Error::syntax("Unicode destination exceeds 512 bytes"));
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
        // Runs of codes whose destinations are all valid, or all invalid.
        let mut run: Option<(Code, Text)> = None;
        let mut code = range.first;
        loop {
            context.budget.work.charge(bytes.len())?;
            let valid = bytes.len().is_multiple_of(2) && utf16(&bytes).is_some();
            if !valid {
                context.report.repair(
                    repairs::TOUNICODE_UTF16,
                    "invalid UTF-16BE destination omitted (9.10.3)",
                    position,
                )?;
            }
            let same = run.as_ref().is_some_and(|(_, text)| match text {
                Text::Absent => !valid,
                Text::Increment(_) | Text::Fixed(_) => valid,
            });
            if !same {
                if let Some((start, text)) = run.take() {
                    let end = Code {
                        value: code.value - 1,
                        len: code.len,
                    };
                    self.insert_unicode(start, end, text, context)?;
                }
                let text = if valid {
                    Text::Increment(bytes.clone().into())
                } else {
                    Text::Absent
                };
                run = Some((code, text));
            }
            // Use big-endian byte carry only after reporting the out-of-spec
            // range. A full string overflow ends the range: wrapped
            // destinations are never invented.
            if code.value == range.last.value || !add(&mut bytes, 1) {
                break;
            }
            code.value += 1;
        }
        if let Some((start, text)) = run {
            self.insert_unicode(start, code, text, context)?;
        }
        Ok(())
    }

    fn destination(
        bytes: &[u8],
        context: &mut ParseContext<'_, '_>,
        position: Position,
    ) -> Result<Text> {
        if bytes.len() > MAX_DESTINATION {
            return Err(Error::syntax("Unicode destination exceeds 512 bytes"));
        }
        context.budget.work.charge(bytes.len())?;
        if let Some(text) = bytes
            .len()
            .is_multiple_of(2)
            .then(|| utf16(bytes))
            .flatten()
        {
            return Ok(Text::Fixed(text));
        }
        context.report.repair(
            repairs::TOUNICODE_UTF16,
            "invalid UTF-16BE destination omitted (9.10.3)",
            position,
        )?;
        Ok(Text::Absent)
    }

    fn insert_unicode(
        &mut self,
        first: Code,
        last: Code,
        text: Text,
        context: &mut ParseContext<'_, '_>,
    ) -> Result<()> {
        // A one-code run needs no increment: store its text once.
        let text = match text {
            Text::Increment(base) if first == last => {
                utf16(&base).map_or(Text::Absent, Text::Fixed)
            }
            other => other,
        };
        context.budget.entries.charge(1)?;
        let budget = &mut *context.budget;
        self.unicode
            .insert(first, last, text, &mut budget.work, &mut budget.text)
    }
}

/// Adds `offset` to a big-endian byte string, with carry. Returns false
/// when the sum does not fit, leaving `bytes` unspecified.
fn add(bytes: &mut [u8], offset: u32) -> bool {
    let mut carry = offset;
    for byte in bytes.iter_mut().rev() {
        if carry == 0 {
            return true;
        }
        let [low, ..] = carry.to_le_bytes();
        let (sum, overflow) = byte.overflowing_add(low);
        *byte = sum;
        carry = (carry >> 8) + u32::from(overflow);
    }
    carry == 0
}

/// Decodes UTF-16BE text, or `None` when it is not valid.
fn utf16(bytes: &[u8]) -> Option<GlyphText> {
    let (units, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let mut valid = true;
    let text = GlyphText::from_chars(
        char::decode_utf16(units.iter().map(|unit| u16::from_be_bytes(*unit)))
            .map_while(|scalar| scalar.inspect_err(|_| valid = false).ok()),
    );
    valid.then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addition_carries_and_detects_overflow() {
        let mut bytes = [0x00, 0xfe];
        assert!(add(&mut bytes, 3));
        assert_eq!(bytes, [0x01, 0x01]);
        let mut bytes = [0xff, 0xff];
        assert!(!add(&mut bytes, 1));
        let mut bytes = [0x12, 0x34];
        assert!(add(&mut bytes, 0x0101));
        assert_eq!(bytes, [0x13, 0x35]);
        assert!(add(&mut [], 0));
        assert!(!add(&mut [], 1));
    }

    #[test]
    fn utf16_rejects_surrogates_and_odd_lengths() {
        assert_eq!(utf16(&[0, 0x41]).as_deref(), Some("A"));
        assert_eq!(utf16(&[0xd8, 0x3d, 0xde, 0x00]).as_deref(), Some("😀"));
        assert_eq!(utf16(&[0xd8, 0x00]), None);
        assert_eq!(utf16(&[0x00]), None);
        assert_eq!(utf16(&[]).as_deref(), Some(""));
    }

    #[test]
    fn overriding_splits_increment_runs_with_adjusted_bases() -> Result<()> {
        let mut map = UnicodeMap::default();
        let mut work = Meter::new(usize::MAX, "test");
        let mut bytes = Meter::new(usize::MAX, "test");
        let code = |value| Code { value, len: 1 };
        map.insert(
            code(0x41),
            code(0x45),
            Text::Increment([0, 0x61].into()),
            &mut work,
            &mut bytes,
        )?;
        map.insert(
            code(0x43),
            code(0x43),
            Text::Fixed("X".into()),
            &mut work,
            &mut bytes,
        )?;
        let texts: Vec<Option<String>> = (0x41..=0x46)
            .map(|value| match map.get(code(value)) {
                Lookup::Text(text) => Some(text.to_string()),
                Lookup::Absent | Lookup::Missing => None,
            })
            .collect();
        let expected = [Some("a"), Some("b"), Some("X"), Some("d"), Some("e"), None];
        assert_eq!(texts, expected.map(|text| text.map(str::to_owned)));
        assert_eq!(map.len(), 3);
        Ok(())
    }
}
