//! Vertical one-bit words and exact column-prefix counts for tiled opacity.
//!
//! Interior sums replace only additions of exactly zero or one. Endpoints
//! and their additions retain the separable row reducer's operation order.

use super::geometry::Footprint;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Result};
use pagura_image::DecodedTiles;

const ARC_BYTES: usize = 2 * size_of::<usize>();

#[cfg(test)]
std::thread_local! {
    static CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_construction_count() {
    CONSTRUCTIONS.set(0);
}

#[cfg(test)]
pub(super) fn construction_count() -> usize {
    CONSTRUCTIONS.get()
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Polarity {
    Forward,
    Reverse,
}

#[derive(Debug)]
pub(super) struct ColumnIndex {
    width: u32,
    height: u32,
    words: Vec<u64>,
    prefixes: Vec<u32>,
}

/// Allocation-free checked bounds, before choosing an indexed plan.
#[derive(Debug, Clone, Copy)]
pub(super) struct Layout {
    width: u32,
    height: u32,
    words: usize,
    prefixes: usize,
    pub(super) bytes: usize,
    pub(super) work: usize,
}

impl Layout {
    pub(super) fn new(width: u32, height: u32) -> Result<Self> {
        let overflow = || Error::limit("tiled column index size overflow");
        if width == 0 || height == 0 {
            return Err(Error::invalid_argument("empty tiled column index"));
        }
        let (w, h) = (u32_to_usize(width), u32_to_usize(height));
        let blocks = h.div_ceil(64);
        let words = w.checked_mul(blocks).ok_or_else(overflow)?;
        let prefixes = words.checked_add(w).ok_or_else(overflow)?;
        let word_bytes = words.checked_mul(size_of::<u64>()).ok_or_else(overflow)?;
        let prefix_bytes = prefixes
            .checked_mul(size_of::<u32>())
            .ok_or_else(overflow)?;
        let bytes = word_bytes
            .checked_add(prefix_bytes)
            .and_then(|n| n.checked_add(size_of::<ColumnIndex>() + ARC_BYTES))
            .ok_or_else(overflow)?;
        // Reject allocations beyond Vec's addressable byte range up front.
        if word_bytes > isize::MAX.unsigned_abs() || prefix_bytes > isize::MAX.unsigned_abs() {
            return Err(overflow());
        }
        let work = w
            .checked_mul(h)
            .and_then(|n| n.checked_mul(6))
            .and_then(|n| words.checked_mul(8).and_then(|m| n.checked_add(m)))
            .and_then(|n| h.checked_mul(16).and_then(|m| n.checked_add(m)))
            .and_then(|n| blocks.checked_mul(8).and_then(|m| n.checked_add(m)))
            .and_then(|n| n.checked_add(w))
            .ok_or_else(overflow)?;
        Ok(Self {
            width,
            height,
            words,
            prefixes,
            bytes,
            work,
        })
    }
}

impl ColumnIndex {
    /// The caller has admitted construction work. Every present logical bit
    /// is copied once; row padding and unfilled final vertical bits are zero.
    pub(super) fn new(tiles: &DecodedTiles, bytes: &mut Meter) -> Result<Self> {
        #[cfg(test)]
        CONSTRUCTIONS.set(CONSTRUCTIONS.get().saturating_add(1));
        let layout = Layout::new(tiles.width(), tiles.height())?;
        Self::build(layout, bytes, |y| tiles.row(y))
    }

    fn build<'a>(
        layout: Layout,
        bytes: &mut Meter,
        mut row: impl FnMut(u32) -> Option<&'a [u8]>,
    ) -> Result<Self> {
        let mut words = Vec::new();
        let mut prefixes = Vec::new();
        // No overlap with a replaced allocation: each vector starts empty.
        bytes.charge(size_of::<Self>() + ARC_BYTES)?;
        reserve(&mut words, layout.words, bytes)?;
        words.resize(layout.words, 0_u64);
        reserve(&mut prefixes, layout.prefixes, bytes)?;
        let w = u32_to_usize(layout.width);
        let stride = w.div_ceil(8);
        for y in 0..layout.height {
            let source = row(y)
                .filter(|source| source.len() >= stride)
                .ok_or(Error::syntax("incomplete tiled column index row"))?;
            let start = u32_to_usize(y / 64) * w;
            let shift = y % 64;
            for x in 0..w {
                let bit = (source[x / 8] >> (7 - x % 8)) & 1;
                words[start + x] |= u64::from(bit) << shift;
            }
        }
        prefixes.resize(w, 0_u32);
        for block in 0..u32_to_usize(layout.height).div_ceil(64) {
            let start = block * w;
            for x in 0..w {
                let count = prefixes[start + x]
                    .checked_add(words[start + x].count_ones())
                    .ok_or(Error::limit("tiled column count overflow"))?;
                prefixes.push(count);
            }
        }
        Ok(Self {
            width: layout.width,
            height: layout.height,
            words,
            prefixes,
        })
    }

    pub(super) fn bytes(&self) -> usize {
        size_of::<Self>()
            + ARC_BYTES
            + self.words.capacity() * size_of::<u64>()
            + self.prefixes.capacity() * size_of::<u32>()
    }

    /// Count one bits in `first..end` of one complete logical column.
    fn ones(&self, x: usize, first: u32, end: u32) -> u32 {
        if first >= end {
            return 0;
        }
        let width = u32_to_usize(self.width);
        let (a, b) = (u32_to_usize(first / 64), u32_to_usize((end - 1) / 64));
        let head = u64::MAX << (first % 64);
        let tail = u64::MAX >> (63 - (end - 1) % 64);
        if a == b {
            return (self.words[a * width + x] & head & tail).count_ones();
        }
        let middle = self.prefixes[b * width + x] - self.prefixes[(a + 1) * width + x];
        (self.words[a * width + x] & head).count_ones()
            + middle
            + (self.words[b * width + x] & tail).count_ones()
    }

    fn bit(&self, x: usize, y: u32, polarity: Polarity) -> f64 {
        let word = self.words[u32_to_usize(y / 64) * u32_to_usize(self.width) + x];
        let bit = ((word >> (y % 64)) & 1) != 0;
        f64::from(u8::from(match polarity {
            Polarity::Forward => bit,
            Polarity::Reverse => !bit,
        }))
    }

    /// Total for all valid separable requests of this immutable grid. The
    /// attached plan has matching dimensions and canonical binary endpoints.
    /// No expensive per-bit fallback occurs after index admission.
    pub(super) fn reduce(
        &self,
        across: &Footprint,
        (first, end): (usize, usize),
        polarity: Polarity,
        out: &mut [f64],
    ) {
        debug_assert!(first <= end && end <= u32_to_usize(self.width));
        debug_assert!(across.first < across.end && across.end <= self.height);
        debug_assert_eq!(out.len(), end - first);
        let top = across.first;
        let bottom = across.end - 1;
        for (x, value) in (first..end).zip(out) {
            let first = across.first_weight * self.bit(x, top, polarity);
            *value = if top == bottom {
                first
            } else if bottom - top == 1 {
                first + across.last_weight * self.bit(x, bottom, polarity)
            } else {
                let ones = self.ones(x, top + 1, bottom);
                let count = match polarity {
                    Polarity::Forward => ones,
                    Polarity::Reverse => bottom - top - 1 - ones,
                };
                (first + f64::from(count)) + across.last_weight * self.bit(x, bottom, polarity)
            };
        }
    }
}

fn reserve<T>(values: &mut Vec<T>, count: usize, bytes: &mut Meter) -> Result<()> {
    bytes.reserve(values, count)?;
    // Exact reservation is permitted to return extra capacity. Charge it
    // before any following allocation and include it in retained ownership.
    bytes.charge_items::<T>(values.capacity().saturating_sub(count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_layout_prices_every_buffer_and_construction_step() {
        let layout = Layout::new(34862, 4332).unwrap();
        assert_eq!(
            (layout.words, layout.prefixes, layout.work),
            (2_370_616, 2_405_478, 925_202_750)
        );
        assert_eq!(
            layout.bytes,
            28_586_840 + size_of::<ColumnIndex>() + ARC_BYTES
        );
        assert!(Layout::new(0, 1).is_err());
        assert!(Layout::new(u32::MAX, u32::MAX).is_err());
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<ColumnIndex>(), 56);
        #[cfg(target_arch = "wasm32")]
        assert_eq!(size_of::<ColumnIndex>(), 32);
    }

    #[test]
    fn words_prefixes_and_ranges_match_literal_bits_including_padding() {
        for width in [1_u32, 7, 63, 64, 65, 127, 128, 129] {
            for height in [1_u32, 2, 7, 63, 64, 65, 127, 128, 129] {
                let w = u32_to_usize(width);
                let mut literal = vec![vec![0xff_u8; w.div_ceil(8)]; u32_to_usize(height)];
                for (y, row) in literal.iter_mut().enumerate() {
                    for x in 0..w {
                        if (x * 7 + y * 11) % 13 < 6 {
                            row[x / 8] &= !(1 << (7 - x % 8));
                        }
                    }
                }
                let mut bytes = Meter::new(1 << 24, "test index bytes");
                let index =
                    ColumnIndex::build(Layout::new(width, height).unwrap(), &mut bytes, |y| {
                        literal.get(u32_to_usize(y)).map(Vec::as_slice)
                    })
                    .unwrap();
                assert_eq!(bytes.used(), index.bytes());
                for x in 0..w {
                    for first in 0..height {
                        for end in [first + 1, first + (height - first).div_ceil(2), height] {
                            let expected = (first..end)
                                .filter(|&y| {
                                    (literal[u32_to_usize(y)][x / 8] >> (7 - x % 8)) & 1 != 0
                                })
                                .count();
                            assert_eq!(u32_to_usize(index.ones(x, first, end)), expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn construction_uses_complete_decoded_rows_and_refuses_missing_rows() {
        use pagura_core::{Limits, ObjectId};
        use pagura_document::{Document, OpenOptions};
        use pagura_image::{ImageSource, Orientation};
        use pagura_testkit::fixture;
        let document = Document::open(
            fixture("packed_mask_tiles", "small-flate.pdf"),
            &OpenOptions {
                limits: Limits {
                    pixels: 10,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let source = ImageSource::from_xobject_with_tiling(
            &document,
            document.object(ObjectId::new(6, 0)).unwrap(),
        )
        .unwrap();
        let tiles = source
            .decode_tiles(Orientation::Rows, &Limits::default())
            .unwrap();
        let mut bytes = Meter::new(1 << 20, "test index bytes");
        let index = ColumnIndex::new(&tiles, &mut bytes).unwrap();
        assert_eq!(bytes.used(), index.bytes());
        let layout = Layout::new(1, 2).unwrap();
        assert!(
            ColumnIndex::build(layout, &mut bytes, |y| if y == 0 {
                Some(&[0x80])
            } else {
                None
            })
            .is_err()
        );
    }

    #[test]
    fn indexed_lines_reproduce_the_actual_old_separated_steps_bit_for_bit() {
        let width = 7_u32;
        let height = 129_u32;
        let literal: Vec<Vec<u8>> = (0..height)
            .map(|y| {
                let mut byte = 1_u8; // Nonzero padding is excluded.
                for x in 0..width {
                    if (x * 7 + y * 11) % 13 >= 6 {
                        byte |= 1 << (7 - x);
                    }
                }
                vec![byte]
            })
            .collect();
        let mut bytes = Meter::new(1 << 20, "test index bytes");
        let index = ColumnIndex::build(Layout::new(width, height).unwrap(), &mut bytes, |y| {
            literal.get(u32_to_usize(y)).map(Vec::as_slice)
        })
        .unwrap();
        let weights = [
            f64::from_bits(1),
            0.125,
            0.5,
            f64::from_bits(1.0_f64.to_bits() - 1),
            1.0,
        ];
        for polarity in [Polarity::Forward, Polarity::Reverse] {
            for top in [0, 1, 31, 32, 63, 64, 65, 127, 128] {
                for bottom in [top, (top + 1).min(height - 1), height - 1] {
                    for first_weight in weights {
                        for last_weight in weights {
                            let footprint = Footprint {
                                first: top,
                                end: bottom + 1,
                                first_weight,
                                last_weight,
                                normalization: 1.0,
                            };
                            let mut actual = vec![0.0; u32_to_usize(width)];
                            index.reduce(
                                &footprint,
                                (0, u32_to_usize(width)),
                                polarity,
                                &mut actual,
                            );
                            let mut old = vec![0.0; u32_to_usize(width)];
                            let mut between = vec![0.0; u32_to_usize(width)];
                            for y in top..=bottom {
                                let values: Vec<f64> = (0..u32_to_usize(width))
                                    .map(|x| {
                                        let bit = (literal[u32_to_usize(y)][0] >> (7 - x)) & 1;
                                        f64::from(match polarity {
                                            Polarity::Forward => bit,
                                            Polarity::Reverse => 1 - bit,
                                        })
                                    })
                                    .collect();
                                super::super::separable::old_step_for_index_test(
                                    (u32_to_usize(y), u32_to_usize(top), u32_to_usize(bottom)),
                                    (first_weight, last_weight),
                                    (&mut old, &mut between),
                                    &values,
                                );
                            }
                            assert_eq!(
                                actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                                old.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn index_eligibility_uses_raw_decode_bits_not_clipped_table_values() {
        use super::super::texels::Opacity;
        let mut bytes = Meter::new(1 << 20, "test opacity bytes");
        for (decode, expected) in [
            ([0.0, 1.0], Some(0)),
            ([1.0, 0.0], Some(1)),
            ([-0.0, 1.0], None),
            ([1.0, -0.0], None),
            ([-100.0, 100.0], None),
            ([100.0, -100.0], None),
            ([-1.0, 1.0], None),
            ([0.25, 0.75], None),
            ([0.0, 0.0], None),
            ([1.0, 1.0], None),
            ([1e300, -1e300], None),
        ] {
            let opacity = Opacity::tiled(decode, &mut bytes).unwrap();
            let actual = match opacity.polarity() {
                Some(Polarity::Forward) => Some(0),
                Some(Polarity::Reverse) => Some(1),
                None => None,
            };
            assert_eq!(actual, expected, "raw Decode {decode:?}");
        }
        assert!(Opacity::tiled([f64::NAN, 1.0], &mut bytes).is_err());
        assert!(Opacity::tiled([0.0, f64::INFINITY], &mut bytes).is_err());
    }
}

#[cfg(test)]
mod frozen_tests;
