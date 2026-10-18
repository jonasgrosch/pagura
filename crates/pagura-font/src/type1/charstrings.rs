//! Length-delimited CharStrings extraction (Type 1 chapters 6 and 10).
//! Duplicate names keep the first definition as reported recovery policy.
use super::{Tokens, aliases, char_bytes};
use crate::{GlyphId, read::cap, recovery::repair, tags};
use pagura_core::{Diagnostics, Limits, Name, Position, Result};
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Default)]
pub(super) struct CharStrings {
    pub glyphs: Vec<Vec<u8>>,
    pub names: BTreeMap<Name, GlyphId>,
}
impl CharStrings {
    pub fn read(
        &mut self,
        r: &mut Tokens<'_>,
        len_iv: Option<usize>,
        limits: &Limits,
        subr_count: usize,
        aliases: &aliases::Aliases,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        let count = r.integer()?;
        cap(count, limits.objects.min(limits.value_nodes), r.pos)?;
        while let Some(token) = r.next()? {
            if token == b"end" {
                break;
            }
            if let Some(name) = token.strip_prefix(b"/") {
                let at = r.pos - token.len();
                cap(name.len(), limits.string_bytes, at)?;
                let program = r.binary(aliases)?;
                aliases.finish(r, aliases::Assignment::Define)?;
                match self.names.entry(Name::new(name)) {
                    Entry::Occupied(_) => repair(
                        diagnostics,
                        tags::TYPE1_DUPLICATE_GLYPH,
                        "later duplicate Type 1 glyph name ignored",
                        Position::Buffer(at),
                    )?,
                    Entry::Vacant(entry) => {
                        cap(
                            self.glyphs
                                .len()
                                .saturating_add(subr_count)
                                .saturating_add(1),
                            limits.objects.min(limits.value_nodes),
                            at,
                        )?;
                        cap(self.glyphs.len() + 1, count, at)?;
                        let glyph =
                            GlyphId(u32::try_from(self.glyphs.len()).map_err(|_| {
                                crate::read::limit(at, "Type 1 glyph count overflow")
                            })?);
                        self.glyphs.push(char_bytes(program, len_iv, r.pos)?);
                        entry.insert(glyph);
                    }
                }
            }
        }
        Ok(())
    }
}
