//! sfnt heap excluding table bytes already owned by FontProgram's input Arc.
use super::Sfnt;
use crate::memory::{names, vector};

impl Sfnt {
    pub(crate) fn heap_bytes(&self) -> usize {
        vector(&self.locations)
            .saturating_add(self.advances.as_ref().map_or(0, vector))
            .saturating_add(vector(&self.cmaps))
            .saturating_add(names(&self.names.by_name))
            .saturating_add(crate::names::heap_bytes(&self.names.by_glyph))
            .saturating_add(self.cff.as_ref().map_or(0, crate::cff::Cff::heap_bytes))
    }
}
