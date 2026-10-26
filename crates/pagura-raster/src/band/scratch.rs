//! Buffers that shape preparation keeps between the shapes of one render.

use super::edges::Line;
use super::union::{Piece, Resolver};
use std::cell::RefCell;

/// The most bytes [`ShapeScratch`] keeps between shapes: buffers grown past
/// it are dropped after the shape.
const KEPT_BYTES: usize = 64 << 10;

/// Buffers the shapes of one render take turns with, so that each does not
/// make and drop its own: the winding resolver and its rows' pieces and
/// sides.
///
/// A shape counts their capacity as paid, as it does a vector's spare
/// capacity. Their owner keeps what they hold ([`Self::bytes`]) charged to
/// the render for as long as it keeps them.
#[derive(Debug, Default)]
pub struct ShapeScratch {
    kept: RefCell<Kept>,
}

#[derive(Debug, Default)]
pub(crate) struct Kept {
    pub(crate) resolver: Resolver,
    pub(crate) pieces: Vec<Piece>,
    pub(crate) sides: Vec<Line>,
}

impl Kept {
    fn bytes(&self) -> usize {
        self.resolver
            .bytes()
            .saturating_add(self.pieces.capacity() * size_of::<Piece>())
            .saturating_add(self.sides.capacity() * size_of::<Line>())
    }
}

impl ShapeScratch {
    /// The bytes its buffers hold.
    pub fn bytes(&self) -> usize {
        self.kept.borrow().bytes()
    }

    /// Drops its buffers.
    pub fn clear(&self) {
        drop(self.kept.take());
    }

    /// Its buffers for one shape; empty while another shape holds them.
    pub(crate) fn take(&self) -> Kept {
        self.kept.take()
    }

    /// Keeps `kept` for the next shape, unless it holds more than
    /// [`KEPT_BYTES`].
    pub(crate) fn keep(&self, kept: Kept) {
        if kept.bytes() <= KEPT_BYTES {
            self.kept.replace(kept);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_past_the_cap_are_dropped() {
        let scratch = ShapeScratch::default();
        let mut kept = scratch.take();
        kept.sides.reserve_exact(16);
        scratch.keep(kept);
        assert_eq!(scratch.bytes(), 16 * size_of::<Line>());
        let mut kept = scratch.take();
        assert_eq!(scratch.bytes(), 0, "a shape holds them");
        kept.pieces
            .reserve_exact(KEPT_BYTES / size_of::<Piece>() + 1);
        scratch.keep(kept);
        assert_eq!(scratch.bytes(), 0);
    }
}
