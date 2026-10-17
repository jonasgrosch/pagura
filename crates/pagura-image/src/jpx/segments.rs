//! Codeword segmentation shared by packet lengths and pass decoding.

pub(super) const BYPASS: u8 = 1;
pub(super) const RESET: u8 = 2;
pub(super) const TERMINATE: u8 = 4;
pub(super) const CAUSAL: u8 = 8;
pub(super) const PREDICTABLE: u8 = 16;
pub(super) const SYMBOLS: u8 = 32;

pub(super) struct Segment {
    pub first: usize,
    pub passes: usize,
    pub data: Vec<u8>,
    pub base: usize,
    pub spans: Vec<super::io::Span>,
}

pub(super) fn end(first: usize, modes: u8) -> usize {
    if modes & TERMINATE != 0 {
        first + 1
    } else if modes & BYPASS == 0 {
        88
    } else if first < 10 {
        10
    } else if first % 3 == 1 {
        first + 2
    } else {
        first + 1
    }
}

pub(super) fn raw(pass: usize, modes: u8, roi: u8) -> bool {
    modes & BYPASS != 0 && pass >= 10 + 3 * usize::from(roi) && !pass.is_multiple_of(3)
}
