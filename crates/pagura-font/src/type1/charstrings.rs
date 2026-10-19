//! Length-delimited CharStrings extraction (Type 1 chapters 6 and 10).
//! Duplicate names keep the first definition as reported recovery policy.
use super::{Tokens, aliases, char_bytes};
use crate::{
    GlyphId,
    read::{bad, cap},
    recovery::repair,
    repairs,
};
use pagura_core::{Diagnostics, Limits, Name, Position, Result};
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Default)]
pub(super) struct CharStrings {
    pub glyphs: Vec<Vec<u8>>,
    pub names: BTreeMap<Name, GlyphId>,
}
impl CharStrings {
    pub(super) fn normalize(&mut self, at: usize) -> Result<()> {
        if self.glyphs.is_empty() || !self.names.contains_key(b".notdef".as_slice()) {
            return Err(bad(at, "Type 1 font has no .notdef charstring"));
        }
        // .notdef is glyph zero for the public API, regardless of dictionary order.
        let notdef = usize::try_from(self.names[b".notdef".as_slice()].0)
            .map_err(|_| bad(at, "invalid Type 1 glyph index"))?;
        if notdef != 0 {
            self.glyphs.swap(0, notdef);
            for glyph in self.names.values_mut() {
                if glyph.0 == 0 {
                    glyph.0 = u32::try_from(notdef)
                        .map_err(|_| crate::read::limit(at, "glyph index overflow"))?;
                } else if usize::try_from(glyph.0).ok() == Some(notdef) {
                    glyph.0 = 0;
                }
            }
        }
        Ok(())
    }

    pub(super) fn read(
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
                        repairs::TYPE1_DUPLICATE_GLYPH,
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
