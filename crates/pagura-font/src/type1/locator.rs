//! Offsets in the decoded copies of a Type 1 program, mapped back to the
//! program bytes, so that errors and repairs point into the input.
//!
//! Parsing reads three copies: the *flat* data without PFB segment headers,
//! the eexec section decoded from hexadecimal, and the decrypted
//! *plaintext*. Charstrings are positioned in plaintext offsets, with their
//! own lenIV prefix skipped.
use crate::memory::vector;
use pagura_core::{Error, Position};

/// Which copy a `Position::Buffer` offset counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Space {
    Flat,
    Plain,
}

#[derive(Debug, Default)]
pub(super) struct Locator {
    /// PFB segment data: the flat and input offsets of each segment's first
    /// byte, ascending. Empty when the program has no PFB framing.
    segments: Vec<(usize, usize)>,
    /// The flat offset where the eexec section begins.
    section: usize,
    /// Plaintext bytes the eexec cipher discards first (Type 1 7.2).
    discard: usize,
    /// Hexadecimal eexec data: the decoded index and flat offset of every
    /// digit pair that does not directly follow the previous one.
    hex: Option<Vec<(usize, usize)>>,
}

impl Locator {
    pub(super) const fn new(segments: Vec<(usize, usize)>) -> Self {
        Self {
            segments,
            section: 0,
            discard: 0,
            hex: None,
        }
    }

    /// Records where the eexec section starts and how it was decoded.
    pub(super) fn section(
        &mut self,
        start: usize,
        discard: usize,
        hex: Option<Vec<(usize, usize)>>,
    ) {
        self.section = start;
        self.discard = discard;
        self.hex = hex;
    }

    /// The input offset of a flat offset.
    pub(super) fn flat(&self, flat: usize) -> usize {
        let segment = self.segments.partition_point(|&(start, _)| start <= flat);
        segment
            .checked_sub(1)
            .and_then(|i| self.segments.get(i))
            .map_or(flat, |&(start, input)| input.saturating_add(flat - start))
    }

    /// The input offset of a plaintext offset.
    pub(super) fn plain(&self, plain: usize) -> usize {
        let decoded = plain.saturating_add(self.discard);
        let flat = self.hex.as_ref().map_or_else(
            || self.section.saturating_add(decoded),
            |pairs| {
                let pair = pairs.partition_point(|&(index, _)| index <= decoded);
                pair.checked_sub(1)
                    .and_then(|i| pairs.get(i))
                    .map_or(self.section, |&(index, flat)| {
                        flat.saturating_add(2 * (decoded - index))
                    })
            },
        );
        self.flat(flat)
    }

    /// `offset` of `space` as an input position.
    pub(super) fn position(&self, offset: usize, space: Space) -> Position {
        Position::Buffer(match space {
            Space::Flat => self.flat(offset),
            Space::Plain => self.plain(offset),
        })
    }

    /// The error with its buffer offset in `space` moved to the input.
    /// This converts coordinates; it adds no context, so it replaces the
    /// offset instead of using the context builders.
    pub(super) fn place(&self, mut error: Error, space: Space) -> Error {
        if let Position::Buffer(offset) = error.position {
            error.position = self.position(offset, space);
        }
        error
    }

    pub(super) fn heap_bytes(&self) -> usize {
        vector(&self.segments).saturating_add(self.hex.as_ref().map_or(0, vector))
    }
}

#[cfg(test)]
mod tests {
    use super::{Locator, Space};
    use pagura_core::Position;

    #[test]
    fn pfb_segments_and_hex_pairs_map_back_to_the_input() {
        // Two PFB segments: flat 0.. at input 6, flat 100.. at input 112.
        let mut locator = Locator::new(vec![(0, 6), (100, 112)]);
        assert_eq!(locator.flat(5), 11);
        assert_eq!(locator.flat(104), 116);
        // Binary eexec from flat 90 with four discarded bytes.
        locator.section(90, 4, None);
        assert_eq!(locator.plain(0), 6 + 94);
        assert_eq!(locator.plain(10), 116);
        // Hex eexec from flat 90: a line break after 32 digit pairs.
        let mut hex = Locator::new(Vec::new());
        hex.section(90, 4, Some(vec![(0, 90), (32, 155)]));
        assert_eq!(hex.plain(0), 98);
        assert_eq!(hex.plain(28), 155);
        assert_eq!(hex.position(1, Space::Flat), Position::Buffer(1));
    }
}
