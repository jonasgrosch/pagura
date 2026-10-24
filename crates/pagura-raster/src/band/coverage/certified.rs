//! Complete, call-local coverage whose bytes preparation has certified.
//!
//! Missing spans mean zero, rather than falling back to another body. The
//! owner uses the common coverage representation and common row output; it
//! retains neither the inspected geometry nor a pixel-correction layer.

use super::{BandCoverage, Capacity, FULL, RowOut};
use crate::band::{Area, DeviceRect};
use pagura_core::{Error, Meter, Result};

/// Complete authoritative coverage, constructed in device row/column order.
/// Only preparation can construct or publish this owner.
#[derive(Debug, Default)]
pub(crate) struct CertifiedRows {
    coverage: BandCoverage,
    x: [u32; 2],
    // These counts already fit the coverage's u32 span/cell addressing.
    // Keeping them compact leaves the existing Body footprint unchanged
    // on both 32-bit and 64-bit targets.
    capacity: [u32; 3],
    work: usize,
}

impl CertifiedRows {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Appends a final byte range. Nonzero ranges must be ordered and
    /// disjoint. Zero coverage has no stored representation.
    pub(crate) fn push_range(
        &mut self,
        y: u32,
        x0: u32,
        x1: u32,
        value: u8,
        work: &mut Meter,
        bytes: &mut Meter,
    ) -> Result<()> {
        work.charge(6)?;
        if x0 > x1 {
            return Err(Error::invalid_argument("reversed certified coverage range"));
        }
        if x0 == x1 || value == 0 {
            return Ok(());
        }
        if y == u32::MAX {
            return Err(Error::limit("certified coverage row exceeds u32"));
        }
        let previous = self.coverage.rows.last();
        if previous.is_some_and(|row| row.y > y) {
            return Err(Error::invalid_argument("unordered certified coverage rows"));
        }
        let new_row = previous.is_none_or(|row| row.y != y);
        let last = previous
            .filter(|_| !new_row)
            .and_then(|_| self.coverage.spans.last());
        if last.is_some_and(|span| span.x + span.len > x0) {
            return Err(Error::invalid_argument(
                "overlapping certified coverage ranges",
            ));
        }
        let full = value == u8::MAX;
        let merge =
            last.is_some_and(|span| span.x + span.len == x0 && (span.cells == FULL) == full);
        let len = x1 - x0;
        let extra_span = usize::from(!merge);
        checked_len(self.coverage.spans.len(), extra_span)?;
        if !full {
            checked_len(self.coverage.cells.len(), len as usize)?;
        }
        reserve(&mut self.coverage.rows, usize::from(new_row), work, bytes)?;
        reserve(&mut self.coverage.spans, extra_span, work, bytes)?;
        if !full {
            reserve(&mut self.coverage.cells, len as usize, work, bytes)?;
            work.charge(len as usize)?;
        }
        work.charge(usize::from(new_row) + extra_span)?;
        if self.coverage.rows.is_empty() {
            self.x = [x0, x1];
        } else {
            self.x = [self.x[0].min(x0), self.x[1].max(x1)];
        }
        if new_row {
            self.coverage.push_row(y);
        }
        self.coverage.put_run(x0, len, value);
        self.capacity = [0; 3];
        self.work = 0;
        Ok(())
    }

    /// Validates the area and computes the largest output band and a work
    /// bound for seeking and emitting every band through either RowOut.
    /// Call after the last range, before publishing the owner.
    pub(crate) fn finish(&mut self, area: &Area, work: &mut Meter) -> Result<()> {
        work.charge(1)?;
        self.capacity = [0; 3];
        self.work = 0;
        let Some(bounds) = self.bounds() else {
            return Ok(());
        };
        if area.band_height == 0 || bounds.intersect(&area.region) != bounds {
            return Err(Error::invalid_argument(
                "certified coverage outside render area",
            ));
        }
        // One-row bands maximize seeking, including gaps between nonzero
        // rows. Also reserve one run-slot setup per intersecting band:
        // rasterize_run counts each slot anew, whereas the common glyph
        // merge allowance includes that scalar setup only once. Both are
        // bounded by the nonzero height, independently of band height.
        let mut emission = (bounds.height() as usize)
            .checked_mul(self.search_work() + 2)
            .ok_or(Error::limit("certified coverage work exceeds usize"))?;
        let mut band = bounds.y0 / area.band_height;
        let mut count = [0_u32; 3];
        for row in &self.coverage.rows {
            work.charge(1)?;
            let own = row.y / area.band_height;
            if own != band {
                self.capacity = max_counts(self.capacity, count);
                count = [0; 3];
                band = own;
            }
            count[0] = count[0]
                .checked_add(1)
                .ok_or(Error::limit("certified coverage rows exceed u32"))?;
            count[1] = count[1]
                .checked_add(row.end - row.start)
                .ok_or(Error::limit("certified coverage spans exceed u32"))?;
            emission = add_work(emission, 2)?;
            for span in &self.coverage.spans[row.start as usize..row.end as usize] {
                work.charge(1)?;
                if span.cells != FULL {
                    count[2] = count[2]
                        .checked_add(span.len)
                        .ok_or(Error::limit("certified coverage cells exceed u32"))?;
                }
                emission = add_work(emission, span_work(span.len, span.cells == FULL))?;
            }
        }
        self.capacity = max_counts(self.capacity, count);
        self.work = emission;
        Ok(())
    }

    /// The actual nonzero byte bounds, not an approximation's support.
    pub(crate) fn bounds(&self) -> Option<DeviceRect> {
        Some(DeviceRect {
            x0: self.x[0],
            x1: self.x[1],
            y0: self.coverage.rows.first()?.y,
            y1: self.coverage.rows.last()?.y + 1,
        })
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.coverage.retained_bytes()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.heap_bytes()
    }

    pub(crate) fn work(&self) -> usize {
        self.work
    }

    pub(crate) fn capacity(&self) -> Capacity {
        Capacity {
            rows: self.capacity[0] as usize,
            spans: self.capacity[1] as usize,
            cells: self.capacity[2] as usize,
        }
    }

    /// Emits complete, already clipped coverage. No source geometry or
    /// clip classification is consulted after publication.
    pub(crate) fn emit(&self, rows: std::ops::Range<u32>, out: &mut dyn RowOut) -> usize {
        let first = self.coverage.rows.partition_point(|row| row.y < rows.start);
        let mut spent = self.search_work() + 1;
        for row in &self.coverage.rows[first..] {
            if row.y >= rows.end {
                break;
            }
            spent = spent.saturating_add(2);
            out.push_row(row.y);
            for span in &self.coverage.spans[row.start as usize..row.end as usize] {
                let full = span.cells == FULL;
                spent = spent.saturating_add(span_work(span.len, full));
                if full {
                    out.put_run(span.x, span.len, u8::MAX);
                } else {
                    let start = span.cells as usize;
                    out.put_row(
                        span.x,
                        &self.coverage.cells[start..start + span.len as usize],
                    );
                }
            }
        }
        spent
    }

    fn search_work(&self) -> usize {
        self.coverage.rows.len().max(1).ilog2() as usize + 2
    }
}

fn checked_len(len: usize, extra: usize) -> Result<()> {
    len.checked_add(extra)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(Error::limit("certified coverage storage exceeds u32"))?;
    Ok(())
}

/// Capacity growth charges the old/replacement overlap, actual resulting
/// capacity and a possible move of the entire old allocation before use.
fn reserve<T>(
    values: &mut Vec<T>,
    extra: usize,
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<()> {
    let old = values.capacity();
    let need = values
        .len()
        .checked_add(extra)
        .ok_or(Error::limit("certified coverage capacity exceeds usize"))?;
    if need <= old {
        return Ok(());
    }
    let preferred = need.max(old.saturating_mul(2));
    let target = if preferred
        .checked_mul(size_of::<T>())
        .is_some_and(|n| n <= bytes.remaining())
    {
        preferred
    } else {
        need
    };
    work.charge(old)?;
    let len = values.len();
    bytes.charge_items::<T>(len)?;
    bytes.reserve(values, target - len)?;
    bytes.charge_items::<T>(values.capacity().saturating_sub(target))?;
    bytes.refund(old.saturating_mul(size_of::<T>()));
    Ok(())
}

fn max_counts(a: [u32; 3], b: [u32; 3]) -> [u32; 3] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
}

fn add_work(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or(Error::limit("certified coverage work exceeds usize"))
}

/// Full spans can expand in SlotOut; partial spans can scan and copy in
/// BandCoverage::put_row as well as copy in SlotOut. Both outputs fit this
/// common bound, including their per-span bounds/address/merge work.
fn span_work(len: u32, full: bool) -> usize {
    (len as usize)
        .saturating_mul(if full { 1 } else { 4 })
        .saturating_add(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (CertifiedRows, Area, usize) {
        let area = Area {
            region: DeviceRect {
                x0: 10,
                y0: 17,
                x1: 20,
                y1: 26,
            },
            band_height: 4,
        };
        let mut work = Meter::new(1 << 20, "work");
        let mut bytes = Meter::new(1 << 20, "bytes");
        let mut rows = CertifiedRows::new();
        for (y, x0, x1, value) in [
            (17, 11, 12, 255),
            (17, 12, 13, 255),
            (17, 13, 14, 7),
            (17, 14, 16, 8),
            (17, 16, 18, 0),
            (17, 18, 19, 9),
            (20, 12, 15, 4),
            (24, 10, 14, 255),
        ] {
            rows.push_range(y, x0, x1, value, &mut work, &mut bytes)
                .unwrap();
        }
        rows.finish(&area, &mut work).unwrap();
        (rows, area, bytes.used())
    }

    fn prepared(rows: CertifiedRows, area: Area) -> crate::Shape {
        let limits = pagura_core::Limits::default();
        let bounds = rows.bounds().unwrap();
        let mut path = crate::PathBuilder::new();
        path.rect(
            f64::from(bounds.x0),
            f64::from(bounds.y0),
            f64::from(bounds.width()),
            f64::from(bounds.height()),
        );
        let mut shape = crate::Shape::fill(
            &path.finish(),
            crate::Transform::IDENTITY,
            crate::FillRule::NonZero,
            &crate::ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap()
        .unwrap()
        .with_certified_rows(rows);
        // The existing placement allowance, unchanged by row publication.
        shape.add_work(bounds.height() as usize * (2 * bounds.width() as usize + 2) + 1);
        shape
    }

    fn emitted(rows: &CertifiedRows, area: &Area) -> (Vec<(u32, u32, u8)>, usize) {
        let mut scratch = Meter::new(1 << 20, "scratch");
        let mut output = BandCoverage::with_capacity(rows.capacity(), &mut scratch).unwrap();
        let capacities = (
            output.rows.capacity(),
            output.spans.capacity(),
            output.cells.capacity(),
        );
        let mut emitted = Vec::new();
        let mut spent = 0;
        let bounds = rows.bounds().unwrap();
        for band in area.bands() {
            output.clear();
            let window = area.band_rows(band);
            let start = window.start.max(bounds.y0);
            let end = window.end.min(bounds.y1).max(start);
            if start < end {
                spent += rows.emit(start..end, &mut output);
            }
            output.finish();
            for row in output.rows() {
                for span in row.spans() {
                    match span {
                        super::super::Span::Full { x, len } => {
                            emitted.extend((x..x + len).map(|x| (row.y, x, 255)));
                        }
                        super::super::Span::Partial { x, coverage } => {
                            emitted.extend((x..).zip(coverage).map(|(x, &v)| (row.y, x, v)));
                        }
                    }
                }
            }
            assert_eq!(
                capacities,
                (
                    output.rows.capacity(),
                    output.spans.capacity(),
                    output.cells.capacity(),
                )
            );
        }
        (emitted, spent)
    }

    #[test]
    fn certified_rows_keep_full_partial_and_zero_gaps_with_exact_band_capacity() {
        let (rows, area, allocated) = fixture();
        assert_eq!(
            rows.bounds(),
            Some(DeviceRect {
                x0: 10,
                y0: 17,
                x1: 19,
                y1: 25,
            })
        );
        assert_eq!(
            rows.capacity(),
            Capacity {
                rows: 1,
                spans: 3,
                cells: 4,
            }
        );
        assert_eq!(rows.heap_bytes(), allocated);
        let (actual, spent) = emitted(&rows, &area);
        let expected = [
            (17, 11, 255),
            (17, 12, 255),
            (17, 13, 7),
            (17, 14, 8),
            (17, 15, 8),
            (17, 18, 9),
            (20, 12, 4),
            (20, 13, 4),
            (20, 14, 4),
            (24, 10, 255),
            (24, 11, 255),
            (24, 12, 255),
            (24, 13, 255),
        ];
        assert_eq!(actual, expected);
        assert!(spent <= rows.work());
    }

    #[test]
    fn certified_rows_work_and_pixels_do_not_depend_on_band_height() {
        let (mut rows, mut area, _) = fixture();
        let expected = emitted(&rows, &area).0;
        let bound = rows.work();
        let mut work = Meter::new(1 << 20, "work");
        for height in [1, 3, 4, 7, 32] {
            area.band_height = height;
            rows.finish(&area, &mut work).unwrap();
            let (actual, spent) = emitted(&rows, &area);
            assert_eq!(actual, expected);
            assert_eq!(rows.work(), bound);
            assert!(spent <= bound);
        }
    }

    #[test]
    fn certified_rows_work_covers_run_slot_setup_in_every_band() {
        for height in [1, 3, 4, 7, 32] {
            let (mut rows, mut area, _) = fixture();
            area.band_height = height;
            rows.finish(&area, &mut Meter::new(1 << 20, "work"))
                .unwrap();
            let (expected, _) = emitted(&rows, &area);
            let bounds = rows.bounds().unwrap();
            let shape = prepared(rows, area);
            let mut bytes = crate::testing::bytes();
            let mut raster = crate::BandRasterizer::new(area, &mut bytes).unwrap();
            let mut output = BandCoverage::with_capacity(shape.capacity(), &mut bytes).unwrap();
            let retained = output.retained_bytes();
            let mut actual = Vec::new();
            for band in shape.bands(&area) {
                assert_eq!(raster.rasterize_run(&[&shape], None, band, &mut output), 1);
                assert_eq!(output.retained_bytes(), retained);
                for row in output.rows() {
                    for span in row.spans() {
                        match span {
                            super::super::Span::Full { x, len } => {
                                actual.extend((x..x + len).map(|x| (row.y, x, 255)));
                            }
                            super::super::Span::Partial { x, coverage } => {
                                actual.extend((x..).zip(coverage).map(|(x, &v)| (row.y, x, v)));
                            }
                        }
                    }
                }
            }
            assert_eq!(actual, expected);
            assert!(raster.work_done() <= shape.work());
            if height == 1 {
                assert!(raster.work_done() > shape.work() - bounds.height() as usize);
            }
        }
    }

    #[test]
    fn certified_runs_publish_separated_stretches_without_empty_rows() {
        for height in [1, 3, 4, 7, 32] {
            let area = Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: 16,
                    y1: 12,
                },
                band_height: height,
            };
            let build = |ranges: &[(u32, u32, u32, u8)]| {
                let mut work = Meter::new(1 << 20, "work");
                let mut bytes = Meter::new(1 << 20, "bytes");
                let mut rows = CertifiedRows::new();
                for &(y, x0, x1, value) in ranges {
                    rows.push_range(y, x0, x1, value, &mut work, &mut bytes)
                        .unwrap();
                }
                rows.finish(&area, &mut work).unwrap();
                prepared(rows, area)
            };
            let shapes = [
                build(&[(2, 1, 3, 255), (5, 2, 3, 17), (7, 1, 2, 255)]),
                build(&[(2, 8, 10, 9), (4, 9, 10, 255), (7, 8, 9, 4)]),
            ];
            let mut bytes = crate::testing::bytes();
            let mut raster = crate::BandRasterizer::new(area, &mut bytes).unwrap();
            // Production run output has the dense regional capacity, rather
            // than the compact capacity of either individual shape.
            let mut output =
                BandCoverage::with_capacity(Capacity::band(&area), &mut bytes).unwrap();
            let retained = output.retained_bytes();
            let mut actual = Vec::new();
            for band in area.bands() {
                assert_eq!(
                    raster.rasterize_run(&[&shapes[0], &shapes[1]], None, band, &mut output),
                    2,
                );
                assert_eq!(output.retained_bytes(), retained);
                for row in output.rows() {
                    for span in row.spans() {
                        match span {
                            super::super::Span::Full { x, len } => {
                                actual.extend((x..x + len).map(|x| (row.y, x, 255)));
                            }
                            super::super::Span::Partial { x, coverage } => {
                                actual.extend((x..).zip(coverage).map(|(x, &v)| (row.y, x, v)));
                            }
                        }
                    }
                }
            }
            assert_eq!(
                actual,
                [
                    (2, 1, 255),
                    (2, 2, 255),
                    (2, 8, 9),
                    (2, 9, 9),
                    (4, 9, 255),
                    (5, 2, 17),
                    (7, 1, 255),
                    (7, 8, 4),
                ]
            );
            assert!(raster.work_done() <= shapes.iter().map(crate::Shape::work).sum::<usize>());
        }
    }

    #[test]
    fn certified_rows_refuse_order_area_and_unpaid_partial_writes() {
        let mut rows = CertifiedRows::default();
        let mut work = Meter::new(1 << 20, "work");
        let mut bytes = Meter::new(1 << 20, "bytes");
        rows.push_range(4, 1, 3, 7, &mut work, &mut bytes).unwrap();
        assert!(rows.push_range(3, 4, 5, 8, &mut work, &mut bytes).is_err());
        assert!(rows.push_range(4, 2, 4, 8, &mut work, &mut bytes).is_err());
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 2,
                y1: 10,
            },
            band_height: 2,
        };
        assert!(rows.finish(&area, &mut work).is_err());
        assert_eq!(rows.work(), 0);

        let mut rows = CertifiedRows::new();
        let mut no_writes = Meter::new(6, "work");
        let mut bytes = Meter::new(1 << 20, "bytes");
        assert!(
            rows.push_range(0, 0, 4, 1, &mut no_writes, &mut bytes)
                .is_err()
        );
        assert_eq!(rows.bounds(), None);
        assert!(rows.coverage.rows.is_empty());
        assert!(rows.coverage.spans.is_empty());
        assert_eq!(rows.coverage.cells, [] as [u8; 0]);
        assert_eq!(rows.heap_bytes(), bytes.used());
    }

    #[test]
    fn certified_rows_leave_zero_coverage_empty_and_charge_partial_owner_on_refusal() {
        let mut rows = CertifiedRows::new();
        let mut work = Meter::new(1 << 20, "work");
        let mut bytes = Meter::new(size_of::<super::super::RowSpans>(), "bytes");
        rows.push_range(0, 0, 8, 0, &mut work, &mut bytes).unwrap();
        assert_eq!(rows.bounds(), None);
        assert_eq!(bytes.used(), 0);
        assert!(
            rows.push_range(0, 0, 8, 255, &mut work, &mut bytes)
                .is_err()
        );
        assert_eq!(rows.bounds(), None);
        assert_eq!(rows.heap_bytes(), bytes.used());
    }
}
