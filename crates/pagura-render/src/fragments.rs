//! Glyph masks from reused row fragments, for [`crate::Accuracy::Competitive`].
//!
//! Each glyph outline is flattened once per linear map and resolution
//! ([`geometry`]). Its edges are clipped to rows once per vertical phase, and
//! those fragments are reused across horizontal phases ([`glyphs`]). A mask is
//! a pure function of its key, so retention and eviction change time, never
//! pixels. None of this meets ADR 0004's one-level contract; its accuracy is
//! measured against the competitive accuracy gate.

mod geometry;
mod glyphs;

pub(crate) use glyphs::{Accumulator, GlyphCache};

use crate::RenderOptions;

/// Glyph flattening and vertical origin lattice for a rendering resolution.
///
/// Below 1.5 device pixels per unit (108 dpi), glyph features approach one
/// pixel, so vertical placement keeps 1/128-pixel origins and curves
/// 0.15-pixel chords. From there up, curve pieces within 1/4 pixel become
/// two area-balanced chords and vertical origins snap to 1/16 pixel, which
/// reuses each glyph's row fragments across eight times fewer phases. From 3
/// device pixels per unit (216 dpi), a glyph's horizontal stems span enough
/// rows that a 1/16-pixel vertical offset shows in every stem edge, so
/// vertical origins snap to 1/64 pixel. Each lattice was measured at 72, 150
/// and 300 dpi against the competitive accuracy gate, page by page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Resolution {
    /// Below 1.5 device pixels per unit.
    Low,
    /// From 1.5 to below 3 device pixels per unit.
    High,
    /// At least 3 device pixels per unit.
    Fine,
}

impl Resolution {
    pub(crate) fn new(options: &RenderOptions) -> Self {
        if options.scale < 1.5 {
            Self::Low
        } else if options.scale < 3.0 {
            Self::High
        } else {
            Self::Fine
        }
    }

    /// Vertical glyph origins per device pixel.
    fn vertical_phases(self) -> u16 {
        match self {
            Self::Low => 128,
            Self::High => 16,
            Self::Fine => 64,
        }
    }

    fn glyph_flattening(self) -> geometry::Flattening {
        match self {
            Self::Low => geometry::Flattening::Chords(0.15),
            Self::High | Self::Fine => geometry::Flattening::Balanced(0.25),
        }
    }
}

/// The glyph cache retained by a rendering context, in bytes.
pub(crate) const CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Whether the target lacks a rounding instruction: baseline x86-64 calls a
/// library function for `floor`, `ceil` and `round`.
const NO_ROUNDING_INSTRUCTION: bool =
    cfg!(all(target_arch = "x86_64", not(target_feature = "sse4.1")));

#[cfg(test)]
mod tests;
