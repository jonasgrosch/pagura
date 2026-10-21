//! Differential and invariance tests of the band rasterizer against the
//! exact backends it replaces.

mod clips;
mod fills;
mod strokes;

use super::*;
use crate::{FillRule, Mask, Path, PathBuilder, Point, Transform};
use pagura_core::{Limits, Meter};

/// A deterministic generator for test geometry.
pub(super) struct Lcg(u64);

impl Lcg {
    pub(super) fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from(self.0 >> 33).unwrap()
    }

    /// A value in `[lo, hi)` on a grid of `1 / steps`.
    pub(super) fn grid(&mut self, lo: f64, hi: f64, steps: u32) -> f64 {
        let span = ((hi - lo) * f64::from(steps)) as u32;
        lo + f64::from(self.next() % span.max(1)) / f64::from(steps)
    }

    pub(super) fn unit(&mut self) -> f64 {
        f64::from(self.next()) / f64::from(1_u32 << 31)
    }
}

pub(super) fn area(x0: u32, y0: u32, x1: u32, y1: u32, band_height: u32) -> Area {
    Area {
        region: DeviceRect { x0, y0, x1, y1 },
        band_height,
    }
}

/// The coverage of `mark` over `area`, row-major over the region.
pub(super) fn raster(
    area: &Area,
    shape: Option<&Shape>,
    clip: Option<&ClipShape>,
) -> (Vec<u8>, usize) {
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut raster = BandRasterizer::new(*area, &mut bytes).unwrap();
    // Sized from the shape, filling the coverage never grows it.
    let capacity = shape.map_or_else(|| Capacity::band(area), Shape::capacity);
    let mut band = BandCoverage::with_capacity(capacity, &mut bytes).unwrap();
    let reserved = band.retained_bytes();
    let width = area.region.width() as usize;
    let mut out = vec![0; width * area.region.height() as usize];
    let bands = match shape {
        Some(s) => s.bands(area),
        None => area.bands(),
    };
    for b in bands {
        raster.rasterize(MarkCoverage { shape, clip }, b, &mut band);
        assert_eq!(band.retained_bytes(), reserved, "the coverage grew");
        let mut last_row = None;
        for row in band.rows() {
            assert!(last_row.is_none_or(|y| y < row.y), "rows in order");
            assert!(area.band_rows(b).contains(&row.y), "row {} outside band {b}", row.y);
            last_row = Some(row.y);
            let line = (row.y - area.region.y0) as usize * width;
            let mut last_end = 0;
            for span in row.spans() {
                let (x, values): (u32, Vec<u8>) = match span {
                    Span::Full { x, len } => (x, vec![255; len as usize]),
                    Span::Partial { x, coverage } => {
                        assert!(coverage.iter().all(|&v| v > 0 && v < 255), "{coverage:?}");
                        (x, coverage.to_vec())
                    }
                };
                assert!(x >= area.region.x0 && x as usize + values.len() <= area.region.x1 as usize);
                assert!(x >= last_end, "spans in order");
                last_end = x + values.len() as u32;
                let start = line + (x - area.region.x0) as usize;
                out[start..start + values.len()].copy_from_slice(&values);
            }
        }
    }
    (out, raster.work_done())
}

/// The old exact coverage of a fill over `area`.
pub(super) fn old_fill(area: &Area, path: &Path, transform: Transform, rule: FillRule) -> Vec<u8> {
    let r = area.region;
    Mask::from_path_with_limits(
        r.width(),
        r.height(),
        path,
        transform,
        rule,
        Limits::default(),
        (r.x0, r.y0),
    )
    .unwrap()
    .coverage()
    .to_vec()
}

pub(super) fn options<'a>(area: Area, limits: &'a Limits, clip: Option<&'a ClipShape>) -> ShapeOptions<'a> {
    ShapeOptions {
        area,
        precision: Precision::Exact,
        limits,
        clip,
    }
}

/// Differences between two coverages: (count, largest).
pub(super) fn compare(a: &[u8], b: &[u8]) -> (usize, u8) {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .filter(|(x, y)| x != y)
        .fold((0, 0), |(n, m), (x, y)| (n + 1, m.max(x.abs_diff(*y))))
}

/// Rasterizes `shape` over `area` at several band heights and sub-regions,
/// checks that every one gives the same bytes and stays within the work
/// bound, and returns the coverage.
pub(super) fn invariant(
    area: &Area,
    prepare: &dyn Fn(&ShapeOptions<'_>) -> Option<Shape>,
    clip: Option<&dyn Fn(&ShapeOptions<'_>) -> ClipShape>,
) -> Vec<u8> {
    let limits = Limits::default();
    let mut reference: Option<Vec<u8>> = None;
    let r = area.region;
    let regions = [
        r,
        DeviceRect {
            x0: r.x0 + r.width() / 3,
            y0: r.y0 + r.height() / 4,
            x1: r.x1 - r.width() / 5,
            y1: r.y1 - r.height() / 3,
        },
        DeviceRect {
            x0: r.x0 + 1,
            y0: r.y0 + r.height() / 2,
            x1: r.x0 + 2,
            y1: r.y0 + r.height() / 2 + 1,
        },
    ];
    for height in [1, 3, 16, 32, 256] {
        for region in regions {
            if region.is_empty() {
                continue;
            }
            let sub = Area {
                region,
                band_height: height,
            };
            let clip_shape = clip.map(|c| c(&options(sub, &limits, None)));
            let opts = options(sub, &limits, clip_shape.as_ref());
            let shape = prepare(&opts);
            let (coverage, work) = if let Some(shape) = &shape {
                let (c, w) = raster(&sub, Some(shape), clip_shape.as_ref());
                assert!(w <= shape.work(), "work {w} beyond its bound {}", shape.work());
                (c, w)
            } else {
                (vec![0; (region.width() * region.height()) as usize], 0)
            };
            let _ = work;
            match &reference {
                None => {
                    assert_eq!(region, r);
                    reference = Some(coverage);
                }
                Some(full) => {
                    let width = r.width() as usize;
                    for y in 0..region.height() as usize {
                        let a = &coverage[y * region.width() as usize..(y + 1) * region.width() as usize];
                        let start = (y + (region.y0 - r.y0) as usize) * width + (region.x0 - r.x0) as usize;
                        let b = &full[start..start + region.width() as usize];
                        assert_eq!(a, b, "band height {height}, region {region:?}, row {y}");
                    }
                }
            }
        }
    }
    reference.unwrap_or_default()
}

/// A closed polygon path.
pub(super) fn polygon(points: &[Point]) -> Path {
    let mut path = PathBuilder::new();
    path.move_to(points[0]);
    for &p in &points[1..] {
        path.line_to(p).unwrap();
    }
    path.close();
    path.finish()
}

#[test]
fn scratch_charges_depend_on_the_region_only() {
    let charge = |band_height| {
        let mut bytes = Meter::new(usize::MAX, "bytes");
        BandRasterizer::new(area(3, 5, 640, 480, band_height), &mut bytes).unwrap();
        bytes.used()
    };
    let first = charge(1);
    for height in [5, 16, 32, 256, 480] {
        assert_eq!(charge(height), first);
    }
}
