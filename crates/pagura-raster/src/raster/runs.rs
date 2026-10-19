//! Fill coverage consumed as row runs, keeping rectangles in closed form.

use super::Mask;
use crate::coverage::Rows;

/// One fill's coverage, consumed as nonzero row runs.
#[derive(Debug)]
pub enum Coverage {
    /// Coverage integrated into a dense mask.
    Mask(Mask),
    /// An axis-aligned rectangle within rectangular clips, in closed form.
    Rectangle(Rectangle),
}

impl From<Mask> for Coverage {
    fn from(mask: Mask) -> Self {
        Self::Mask(mask)
    }
}

/// Axis-aligned rectangle coverage without a dense mask. Its runs are the
/// runs of the mask [`super::Rasterizer::clipped_path`] would integrate, but
/// each distinct row is written and split into runs only once.
pub struct Rectangle {
    pub(super) origin: (u32, u32),
    pub(super) area: usize,
    pub(super) rows: Rows,
}

/// Summarizes instead of dumping coverage data.
impl std::fmt::Debug for Rectangle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rectangle")
            .field("origin", &self.origin)
            .field("area", &self.area)
            .finish_non_exhaustive()
    }
}

impl Rectangle {
    /// Global device origin of this coverage region.
    pub fn origin(&self) -> (u32, u32) {
        self.origin
    }

    /// Sample count of the region a mask would cover, as the length of
    /// [`Mask::coverage`] for the same fill.
    pub fn area(&self) -> usize {
        self.area
    }

    /// Nonzero horizontal runs as [`Mask::spans`] yields them for the same
    /// fill, without visiting uncovered or interior samples.
    pub fn spans(&self) -> impl Iterator<Item = (usize, usize, &[u8])> {
        self.rows.spans()
    }
}
