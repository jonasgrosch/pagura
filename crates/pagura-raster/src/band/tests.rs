//! Invariance and consistency tests of the band rasterizer: the same bytes,
//! work and charges at every band height and on sub-regions, and its
//! specialized routes against its general ones. Its exactness against the reference is
//! tested in `tests/exact`.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test geometry is small and nonnegative"
)]

mod clips;
mod fills;
mod glyphs;
mod strokes;

use super::*;
pub(super) use crate::testing::raster;
use crate::{FillRule, Path, PathBuilder, Point, Transform};
use pagura_core::{Limits, Meter};

/// A deterministic generator for test geometry.
pub(super) struct Lcg(pub(super) u64);

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
}

pub(super) fn area(x0: u32, y0: u32, x1: u32, y1: u32, band_height: u32) -> Area {
    Area {
        region: DeviceRect { x0, y0, x1, y1 },
        band_height,
    }
}

pub(super) fn options<'a>(area: Area, limits: &'a Limits, clip: Option<&'a ClipShape>) -> Prep<'a> {
    Prep {
        area,
        limits,
        clip,
        scratch: None,
    }
}

/// The public options of `area`.
pub(super) fn public(area: Area, limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area,
        limits,
        scratch: None,
    }
}

/// Rasterizes `shape` over `area` at several band heights and sub-regions,
/// checks that every one gives the same bytes and stays within the work
/// bound, and returns the coverage.
pub(super) fn invariant(
    area: &Area,
    prepare: &dyn Fn(&Prep<'_>) -> Option<Shape>,
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
    let mut charges: Vec<(DeviceRect, usize, usize)> = Vec::new();
    for height in [1, 3, 5, 16, 32, 256] {
        for region in regions {
            if region.is_empty() {
                continue;
            }
            let sub = Area {
                region,
                band_height: height,
            };
            let clip_shape = clip.map(|c| c(&public(sub, &limits)));
            let opts = options(sub, &limits, clip_shape.as_ref());
            let shape = prepare(&opts);
            let (coverage, work) = if let Some(shape) = &shape {
                let (c, w) = raster(&sub, Some(shape), clip_shape.as_ref());
                assert!(
                    w <= shape.work(),
                    "work {w} beyond its bound {}",
                    shape.work()
                );
                (c, w)
            } else {
                (vec![0; (region.width() * region.height()) as usize], 0)
            };
            let _ = work;
            // Charges depend on the region, never on the band height.
            let charge = (
                shape.as_ref().map_or(0, Shape::work),
                clip_shape.as_ref().map_or(0, ClipShape::work),
            );
            match charges.iter().find(|c| c.0 == region) {
                Some(&(_, w, c)) => assert_eq!((w, c), charge, "charges at band height {height}"),
                None => charges.push((region, charge.0, charge.1)),
            }
            match &reference {
                None => {
                    assert_eq!(region, r);
                    reference = Some(coverage);
                }
                Some(full) => {
                    let width = r.width() as usize;
                    for y in 0..region.height() as usize {
                        let a = &coverage
                            [y * region.width() as usize..(y + 1) * region.width() as usize];
                        let start =
                            (y + (region.y0 - r.y0) as usize) * width + (region.x0 - r.x0) as usize;
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

#[test]
fn a_rasterizer_charges_what_it_reserves() {
    for x1 in [1, 63, 64, 65, 2550] {
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1,
                y1: 40,
            },
            band_height: 16,
        };
        let mut made = Meter::new(usize::MAX, "made");
        let rasterizer = BandRasterizer::new(area, &mut made).unwrap();
        let mut charged = Meter::new(usize::MAX, "charged");
        BandRasterizer::charge(area, &mut charged).unwrap();
        assert_eq!(charged.used(), made.used(), "{x1}");
        assert_eq!(rasterizer.retained_bytes(), made.used(), "{x1}");
    }
}
