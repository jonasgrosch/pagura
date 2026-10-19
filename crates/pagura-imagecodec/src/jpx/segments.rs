//! Codeword segmentation shared by packet lengths and pass decoding.

use pagura_core::{Position, Result};

use crate::budget::Budget;

// Code-block style flags (T.800 Table A.19).
/// Selective arithmetic coding bypass.
pub(super) const BYPASS: u8 = 1;
/// Reset of context probabilities on coding pass boundaries.
pub(super) const RESET: u8 = 2;
/// Termination on each coding pass.
pub(super) const TERMINATE: u8 = 4;
/// Vertically causal context.
pub(super) const CAUSAL: u8 = 8;
/// Predictable termination.
pub(super) const PREDICTABLE: u8 = 16;
/// Segmentation symbols.
pub(super) const SYMBOLS: u8 = 32;
/// The most bit planes a code-block coefficient has in this decoder, whose
/// coefficients hold 30 magnitude bits.
pub(super) const MAX_PLANES: u8 = 30;

/// The most coding passes of a code-block: a cleanup pass for the first
/// bit plane and three passes for each other one (T.800 D.1).
pub(super) const MAX_PASSES: usize = 3 * 30 - 2;

/// Every code-block style flag T.800 defines.
pub(super) const ALL: u8 = BYPASS | RESET | TERMINATE | CAUSAL | PREDICTABLE | SYMBOLS;

/// The codeword segment of some coding passes of a code-block, reassembled
/// from the packets that contributed to it (T.800 B.10.7, D.4).
pub(super) struct Segment {
    pub(super) first: usize,
    pub(super) passes: usize,
    pub(super) data: Vec<u8>,
    /// Where each contribution's bytes came from, by offset in `data`.
    spans: Vec<Span>,
}

#[derive(Clone, Copy)]
struct Span {
    offset: usize,
    source: Position,
}

impl Segment {
    pub(super) const fn new(first: usize) -> Self {
        Self {
            first,
            passes: 0,
            data: Vec::new(),
            spans: Vec::new(),
        }
    }

    /// Appends a packet's contribution, which starts at `source`.
    pub(super) fn append(
        &mut self,
        bytes: &[u8],
        source: Position,
        budget: &mut Budget,
    ) -> Result<()> {
        if !bytes.is_empty() {
            budget.push(
                &mut self.spans,
                Span {
                    offset: self.data.len(),
                    source,
                },
            )?;
        }
        budget.append(&mut self.data, bytes)
    }

    /// The input position of byte `offset` of the reassembled data, or of
    /// the end of the contribution it would follow.
    pub(super) fn position(&self, offset: usize) -> Position {
        let index = self.spans.partition_point(|span| span.offset <= offset);
        index
            .checked_sub(1)
            .and_then(|index| self.spans.get(index))
            .filter(|span| span.source != Position::Unknown)
            .map_or(Position::Unknown, |span| {
                Position::Buffer(offset - span.offset).within(span.source)
            })
    }
}

pub(super) fn end(first: usize, modes: u8) -> usize {
    if modes & TERMINATE != 0 {
        first + 1
    } else if modes & BYPASS == 0 {
        MAX_PASSES
    } else if first < 10 {
        10
    } else if first % 3 == 1 {
        first + 2
    } else {
        first + 1
    }
}

pub(super) fn raw(pass: usize, modes: u8) -> bool {
    modes & BYPASS != 0 && pass >= 10 && !pass.is_multiple_of(3)
}
