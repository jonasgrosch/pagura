//! Length-delimited CharStrings extraction (Type 1 chapters 6 and 10).
//! Duplicate names keep the first definition as reported recovery policy.
use super::{Tokens, aliases, char_bytes};
use crate::{GlyphId, read::bad, repairs};
use pagura_core::{Name, Position, Result};
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
                    glyph.0 = u32::try_from(notdef).map_err(|_| {
                        crate::read::limit(Position::Buffer(at), "glyph index overflow")
                    })?;
                } else if usize::try_from(glyph.0).ok() == Some(notdef) {
                    glyph.0 = 0;
                }
            }
        }
        Ok(())
    }

    pub(super) fn read(
        &mut self,
        r: &mut Tokens<'_, '_, '_>,
        len_iv: Option<usize>,
        aliases: &aliases::Aliases,
    ) -> Result<()> {
        let count = r.integer()?;
        let at = Position::Buffer(r.pos);
        r.work().entries(count, at)?;
        while let Some(token) = r.next()? {
            if token == b"end" {
                break;
            }
            if let Some(name) = token.strip_prefix(b"/") {
                let at = r.pos - token.len();
                let program = r.binary(aliases)?;
                aliases.finish(r, aliases::Assignment::Define)?;
                match self.names.entry(Name::new(name)) {
                    Entry::Occupied(_) => r.work().repair(
                        repairs::TYPE1_DUPLICATE_GLYPH,
                        "later duplicate Type 1 glyph name ignored",
                        Position::Buffer(at),
                    )?,
                    Entry::Vacant(entry) => {
                        if self.glyphs.len() >= count {
                            return Err(bad(at, "more Type 1 charstrings than declared"));
                        }
                        let glyph = GlyphId(u32::try_from(self.glyphs.len()).map_err(|_| {
                            crate::read::limit(Position::Buffer(at), "Type 1 glyph count overflow")
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
