//! Length-delimited CharStrings extraction (Type 1 chapters 6 and 10).
//! Duplicate names keep the first definition as reported recovery policy.
use super::Charstring;
use super::aliases::{Aliases, Assignment};
use super::locator::{Locator, Space};
use super::tokens::Tokens;
use crate::read::{glyph_id, syntax};
use crate::{GlyphId, repairs};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Debug, Default)]
pub(super) struct CharStrings {
    pub(super) glyphs: Vec<Charstring>,
    pub(super) names: BTreeMap<Name, GlyphId>,
}

impl CharStrings {
    pub(super) fn read(
        &mut self,
        r: &mut Tokens<'_, '_, '_>,
        len_iv: Option<usize>,
        aliases: &Aliases,
        locator: &Locator,
    ) -> Result<()> {
        let count = r.integer()?;
        let at = r.position();
        r.work().entries(count, at)?;
        while let Some(token) = r.token()? {
            if token == b"end" {
                break;
            }
            let Some(name) = token.strip_prefix(b"/") else {
                continue;
            };
            let name_at = r.token_start();
            // Errors stay in plaintext offsets until the parse places them;
            // repairs are reported with their input position.
            let at = Position::Buffer(name_at);
            let (program, start) = r.binary(aliases)?;
            aliases.finish(r, Assignment::Define)?;
            match self.names.entry(Name::try_new(name).map_err(|e| e.at(at))?) {
                Entry::Occupied(_) => r.work().repair(
                    repairs::TYPE1_DUPLICATE_GLYPH,
                    "later duplicate Type 1 glyph name ignored",
                    locator.position(name_at, Space::Plain),
                )?,
                Entry::Vacant(entry) => {
                    if self.glyphs.len() >= count {
                        return Err(syntax(at, "more Type 1 charstrings than declared"));
                    }
                    let glyph = glyph_id(self.glyphs.len(), at)?;
                    self.glyphs
                        .push(Charstring::new(start, program.len(), len_iv));
                    entry.insert(glyph);
                }
            }
        }
        Ok(())
    }

    /// Makes `.notdef` glyph zero for the public API, whatever the
    /// dictionary order. `at` locates the CharStrings dictionary.
    pub(super) fn normalize(&mut self, at: Position) -> Result<()> {
        let Some(&notdef) = self
            .names
            .get(b".notdef".as_slice())
            .filter(|_| !self.glyphs.is_empty())
        else {
            return Err(syntax(at, "Type 1 font has no .notdef charstring"));
        };
        let index = u32_to_usize(notdef.0);
        if index != 0 {
            if index >= self.glyphs.len() {
                return Err(syntax(at, "invalid Type 1 glyph index"));
            }
            self.glyphs.swap(0, index);
            for glyph in self.names.values_mut() {
                if glyph.0 == 0 {
                    *glyph = notdef;
                } else if *glyph == notdef {
                    glyph.0 = 0;
                }
            }
        }
        Ok(())
    }
}
