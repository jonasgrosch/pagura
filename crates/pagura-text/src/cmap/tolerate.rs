//! Bounded, reported CMap interpretations; strict mode rejects each deviation.
use super::{CMap, Code, Lookup, ParseContext, Range, UnicodeMap};
use crate::{repairs, report::Report};
use pagura_core::{Error, Meter, Position, Result};
use std::collections::{BTreeMap, BTreeSet};

impl CMap {
    pub(super) fn codespace(
        &mut self,
        range: Range,
        context: &mut ParseContext<'_, '_>,
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
            context.budget.work.charge(1)?;
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

    fn push_space(&mut self, range: Range, context: &mut ParseContext<'_, '_>) -> Result<()> {
        context.budget.entries.charge(1)?;
        self.spaces.push(range);
        Ok(())
    }

    /// A ToUnicode CMap of a simple font with two-byte source codes uses
    /// their low byte (9.10.3), reported. Codes whose low bytes disagree are
    /// dropped. Returns the flattened one-byte map, or `None` when the map
    /// already uses one-byte codes.
    pub(crate) fn simple_compatible(
        &self,
        document: &pagura_document::Document,
        position: Position,
    ) -> Result<Option<Self>> {
        if self.simple_codes() {
            return Ok(None);
        }
        if self.spaces().iter().any(|space| space.first.len > 2) || self.has_long_unicode_codes() {
            return Err(Error::syntax(
                "unsupported simple-font ToUnicode code length",
            ));
        }
        Report::new(document).repair(
            repairs::TOUNICODE_SIMPLE_CODES,
            "simple-font ToUnicode uses two-byte source codes (9.10.3)",
            position,
        )?;
        let mut work = Meter::new(
            document.limits().font_work,
            "simple-font ToUnicode work budget exceeded",
        );
        let mut unicode = BTreeMap::new();
        let mut ambiguous = BTreeSet::new();
        for (code, text) in self.flattened(&mut work)? {
            work.charge(1)?;
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
        let mut flat = UnicodeMap::default();
        for (code, text) in unicode {
            flat.insert_flat(code, text);
        }
        let mut map = Self {
            unicode: flat,
            ..Self::default()
        };
        map.spaces.push(Range {
            first: Code { value: 0, len: 1 },
            last: Code { value: 255, len: 1 },
        });
        Ok(Some(map))
    }

    /// Every code with its text, inherited mappings included and shadowed by
    /// this map's own. Only for maps with one- and two-byte codes.
    fn flattened(&self, work: &mut Meter) -> Result<BTreeMap<Code, Option<crate::GlyphText>>> {
        let mut codes = BTreeMap::new();
        let mut layer = Some(self);
        while let Some(map) = layer {
            for (first, last) in map.unicode.codes() {
                for value in first.value..=last.value {
                    work.charge(1)?;
                    let code = Code {
                        value,
                        len: first.len,
                    };
                    if codes.contains_key(&code) {
                        continue;
                    }
                    let text = match map.unicode.get(code) {
                        Lookup::Text(text) => Some(text),
                        Lookup::Absent | Lookup::Missing => None,
                    };
                    codes.insert(code, text);
                }
            }
            layer = map.parent.as_ref().map(|parent| parent.map.as_ref());
        }
        Ok(codes)
    }
}
