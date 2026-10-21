//! The spans one mark covers in one band, as the band executor consumes them.

use super::Area;
use pagura_core::{Meter, Result};

/// One partial or full run of a coverage row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span<'a> {
    /// Pixels `x .. x + len` fully covered.
    Full {
        /// First column.
        x: u32,
        /// Number of pixels.
        len: u32,
    },
    /// Pixels `x .. x + coverage.len()` with partial coverage, 1..=254 each.
    Partial {
        /// First column.
        x: u32,
        /// Coverage per pixel.
        coverage: &'a [u8],
    },
}

/// The coverage of one mark in one band: per row, its spans left to right.
/// Reused for every mark of a render.
#[derive(Debug, Default)]
pub struct BandCoverage {
    rows: Vec<RowSpans>,
    spans: Vec<RawSpan>,
    cells: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RowSpans {
    y: u32,
    start: u32,
    end: u32,
}

/// The offset of a full span's (absent) cells.
const FULL: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct RawSpan {
    x: u32,
    len: u32,
    /// Offset into `cells`, or [`FULL`] for a full span.
    cells: u32,
}

/// The spans of one row.
#[derive(Debug, Clone, Copy)]
pub struct CoverageRow<'a> {
    /// The device row.
    pub y: u32,
    spans: &'a [RawSpan],
    cells: &'a [u8],
}

impl<'a> CoverageRow<'a> {
    /// The spans, left to right, non-overlapping.
    pub fn spans(&self) -> impl Iterator<Item = Span<'a>> + 'a {
        let cells = self.cells;
        self.spans.iter().map(move |span| {
            if span.cells == FULL {
                Span::Full {
                    x: span.x,
                    len: span.len,
                }
            } else {
                let start = span.cells as usize;
                let end = start + span.len as usize;
                Span::Partial {
                    x: span.x,
                    coverage: cells.get(start..end).unwrap_or_default(),
                }
            }
        })
    }
}

impl BandCoverage {
    /// Storage for the largest coverage one band of `area` can hold, charged
    /// to `bytes`, so that filling it never allocates.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the storage exceeds `bytes` or cannot be
    /// allocated.
    pub fn with_capacity(area: &Area, bytes: &mut Meter) -> Result<Self> {
        let rows = area.band_height.min(area.region.height()) as usize;
        let width = area.region.width() as usize;
        let cells = rows.saturating_mul(width);
        let mut coverage = Self::default();
        bytes.reserve(&mut coverage.rows, rows)?;
        bytes.reserve(&mut coverage.spans, cells)?;
        bytes.reserve(&mut coverage.cells, cells)?;
        Ok(coverage)
    }

    /// Rows with at least one span, top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = CoverageRow<'_>> {
        self.rows.iter().map(|row| CoverageRow {
            y: row.y,
            spans: self
                .spans
                .get(row.start as usize..row.end as usize)
                .unwrap_or_default(),
            cells: &self.cells,
        })
    }

    /// Whether no pixel is covered.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Removes every span, keeping the storage.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.spans.clear();
        self.cells.clear();
    }

    /// Starts row `y`; rows are pushed top to bottom and spans go to the
    /// last row started. A row left without spans is dropped.
    pub fn push_row(&mut self, y: u32) {
        if self.rows.last().is_some_and(|row| row.start == row.end) {
            self.rows.pop();
        }
        let start = u32::try_from(self.spans.len()).unwrap_or(u32::MAX);
        self.rows.push(RowSpans {
            y,
            start,
            end: start,
        });
    }

    /// Appends a fully covered span to the current row; spans are pushed
    /// left to right without overlap. Ignored before the first row.
    pub fn push_full(&mut self, x: u32, len: u32) {
        self.put_run(x, len, u8::MAX);
    }

    /// Appends pixels of partial coverage to the current row, left of any
    /// later span; zero entries end a span. Ignored before the first row.
    pub fn push_partial(&mut self, x: u32, coverage: &[u8]) {
        for (offset, &value) in (0..).zip(coverage) {
            self.put(x.saturating_add(offset), value);
        }
    }

    /// Drops a trailing row without spans; call after the last row.
    pub fn finish(&mut self) {
        if self.rows.last().is_some_and(|row| row.start == row.end) {
            self.rows.pop();
        }
    }

    /// Appends one pixel's coverage to the current row, merging it with the
    /// span before it where they meet: full pixels into full spans, partial
    /// pixels into partial spans. Zero coverage adds nothing.
    #[inline]
    pub(crate) fn put(&mut self, x: u32, value: u8) {
        self.put_run(x, 1, value);
    }

    /// Appends `len` pixels of equal coverage from `x`, as [`Self::put`].
    pub(crate) fn put_run(&mut self, x: u32, len: u32, value: u8) {
        if len == 0 || value == 0 {
            return;
        }
        let Some(row) = self.rows.last_mut() else {
            return;
        };
        let full = value == u8::MAX;
        let last = if row.end > row.start {
            self.spans.last_mut()
        } else {
            None
        };
        if let Some(span) = last
            && span.x.wrapping_add(span.len) == x
            && (span.cells == FULL) == full
        {
            span.len = span.len.wrapping_add(len);
            if !full {
                self.cells.extend(std::iter::repeat_n(value, len as usize));
            }
            return;
        }
        let cells = if full {
            FULL
        } else {
            let Ok(offset) = u32::try_from(self.cells.len()) else {
                return;
            };
            self.cells.extend(std::iter::repeat_n(value, len as usize));
            offset
        };
        self.spans.push(RawSpan { x, len, cells });
        row.end = u32::try_from(self.spans.len()).unwrap_or(u32::MAX);
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.rows
            .capacity()
            .saturating_mul(size_of::<RowSpans>())
            .saturating_add(self.spans.capacity().saturating_mul(size_of::<RawSpan>()))
            .saturating_add(self.cells.capacity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_keep_their_spans_in_order_and_merge_neighbours() {
        let mut band = BandCoverage::default();
        band.push_row(3);
        band.push_partial(1, &[10, 20]);
        band.push_full(3, 4);
        band.put_run(7, 2, 255);
        band.put(9, 0);
        band.put(10, 7);
        band.push_row(4);
        band.push_row(5);
        band.push_full(0, 2);
        band.finish();
        let rows: Vec<_> = band
            .rows()
            .map(|row| (row.y, row.spans().collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    3,
                    vec![
                        Span::Partial {
                            x: 1,
                            coverage: &[10, 20]
                        },
                        Span::Full { x: 3, len: 6 },
                        Span::Partial {
                            x: 10,
                            coverage: &[7]
                        },
                    ]
                ),
                (5, vec![Span::Full { x: 0, len: 2 }]),
            ]
        );
        band.clear();
        assert!(band.is_empty());
    }

    #[test]
    fn capacity_covers_a_band_and_is_charged() {
        let area = Area {
            region: super::super::DeviceRect {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 100,
            },
            band_height: 4,
        };
        let mut bytes = Meter::new(1 << 20, "budget");
        let band = BandCoverage::with_capacity(&area, &mut bytes).unwrap();
        assert!(bytes.used() >= 40);
        assert!(band.retained_bytes() >= 40);
        let mut tiny = Meter::new(8, "budget");
        assert!(BandCoverage::with_capacity(&area, &mut tiny).is_err());
    }
}
