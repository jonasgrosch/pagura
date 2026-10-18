//! Heap reachable through CFF metadata; INDEX ranges borrow the original bytes.
use super::Cff;
use crate::memory::{encoding, names, nested, tree, vector};

impl Cff {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.private
            .iter()
            .fold(vector(&self.private), |sum, p| {
                sum.saturating_add(vector(&p.subrs))
            })
            .saturating_add(vector(&self.glyphs))
            .saturating_add(vector(&self.global))
            .saturating_add(vector(&self.select))
            .saturating_add(names(&self.names))
            .saturating_add(self.cids.as_ref().map_or(0, tree))
            .saturating_add(self.encoding.as_deref().map_or(0, encoding))
            .saturating_add(nested(&self.variations))
    }
}
