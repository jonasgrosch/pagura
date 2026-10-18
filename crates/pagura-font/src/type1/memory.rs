//! Type 1 keeps decrypted charstrings and subroutines, not decrypted dictionaries.
use super::Type1;
use crate::memory::{encoding, names, nested};

impl Type1 {
    pub(crate) fn heap_bytes(&self) -> usize {
        nested(&self.glyphs)
            .saturating_add(nested(&self.subrs))
            .saturating_add(names(&self.names))
            .saturating_add(encoding(&self.encoding))
    }
}
