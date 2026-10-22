//! Geometry generators, rasterization through the public band API and the
//! reference's exact coverage.

use pagura_core::{Limits, Meter};
use pagura_raster::{
    Area, BandCoverage, BandRasterizer, ClipShape, DeviceRect, FillRule, MarkCoverage, Path,
    PathBuilder, Point, Precision, Shape, ShapeOptions, Span, Transform,
};
use pagura_reference::{Region, exact};

/// Clipping paths in device space with their rules.
pub type Clips<'a> = &'a [(&'a Path, FillRule)];

/// A deterministic generator for test geometry.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from(self.0 >> 33).unwrap()
    }

    /// A value in `[lo, hi)` on a grid of `1 / steps`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test spans are small and nonnegative"
    )]
    pub fn grid(&mut self, lo: f64, hi: f64, steps: u32) -> f64 {
        let span = ((hi - lo) * f64::from(steps)) as u32;
        lo + f64::from(self.next() % span.max(1)) / f64::from(steps)
    }
}

pub fn area(x0: u32, y0: u32, x1: u32, y1: u32, band_height: u32) -> Area {
    Area {
        region: DeviceRect { x0, y0, x1, y1 },
        band_height,
    }
}

/// Exact options of `area`.
pub fn options(area: Area, limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area,
        precision: Precision::Exact,
        limits,
    }
}

/// The pixels of a region.
pub fn pixels(region: DeviceRect) -> usize {
    region.width() as usize * region.height() as usize
}

/// The coverage of `shape` within `clip` over `area`, row-major over its
/// region, and the work rasterizing it took.
pub fn raster(area: &Area, shape: Option<&Shape>, clip: Option<&ClipShape>) -> (Vec<u8>, usize) {
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut raster = BandRasterizer::new(*area, &mut bytes).unwrap();
    let capacity = shape.map_or_else(|| pagura_raster::Capacity::band(area), Shape::capacity);
    let mut band = BandCoverage::with_capacity(capacity, &mut bytes).unwrap();
    let region = area.region;
    let width = region.width() as usize;
    let mut out = vec![0; pixels(region)];
    let bands = shape.map_or_else(|| area.bands(), |s| s.bands(area));
    for b in bands {
        raster.rasterize(MarkCoverage { shape, clip }, b, &mut band);
        for row in band.rows() {
            for span in row.spans() {
                let (x, values) = match span {
                    Span::Full { x, len } => (x, vec![255; len as usize]),
                    Span::Partial { x, coverage } => (x, coverage.to_vec()),
                };
                let start = (row.y - region.y0) as usize * width + (x - region.x0) as usize;
                out[start..start + values.len()].copy_from_slice(&values);
            }
        }
    }
    (out, raster.work_done())
}

/// Prepares a mark in the clip it is painted in.
pub type Prepare<'a> = &'a dyn Fn(&ShapeOptions<'_>, Option<&ClipShape>) -> Option<Shape>;

/// Rasterizes the mark `prepare` gives within `clips` over `area` at several
/// band heights and on sub-regions, checks that every one gives the same
/// bytes, stays within the work bound and charges the same for the same
/// region, and returns the coverage.
pub fn invariant(area: &Area, prepare: Prepare<'_>, clips: Clips<'_>) -> Vec<u8> {
    let limits = Limits::default();
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
    let mut whole: Option<Vec<u8>> = None;
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
            let options = options(sub, &limits);
            let clip = (!clips.is_empty())
                .then(|| ClipShape::new(clips, Transform::IDENTITY, &options).unwrap());
            let shape = prepare(&options, clip.as_ref());
            let coverage = match &shape {
                Some(shape) => {
                    let (coverage, work) = raster(&sub, Some(shape), clip.as_ref());
                    assert!(
                        work <= shape.work(),
                        "work {work} beyond its bound {}",
                        shape.work()
                    );
                    coverage
                }
                None => vec![0; pixels(region)],
            };
            let charge = (
                shape.as_ref().map_or(0, Shape::work),
                clip.as_ref().map_or(0, ClipShape::work),
            );
            match charges.iter().find(|c| c.0 == region) {
                Some(&(_, w, c)) => assert_eq!((w, c), charge, "charges at band height {height}"),
                None => charges.push((region, charge.0, charge.1)),
            }
            let Some(full) = &whole else {
                assert_eq!(region, r);
                whole = Some(coverage);
                continue;
            };
            let (width, sub_width) = (r.width() as usize, region.width() as usize);
            for (y, row) in coverage.chunks_exact(sub_width).enumerate() {
                let start = (y + (region.y0 - r.y0) as usize) * width + (region.x0 - r.x0) as usize;
                assert_eq!(
                    row,
                    &full[start..start + sub_width],
                    "band height {height}, region {region:?}, row {y}"
                );
            }
        }
    }
    whole.unwrap_or_default()
}

/// The reference's coverage of `mark` within `clips` over `region`.
pub fn reference(region: DeviceRect, mark: Region, clips: Clips<'_>) -> Vec<u8> {
    let limits = Limits::default();
    let mut regions = vec![mark];
    for &(path, rule) in clips {
        regions.push(exact::fill(path, Transform::IDENTITY, rule, &limits).unwrap());
    }
    exact::coverage(&regions, region).unwrap()
}

/// Differences between two coverages: (count, largest).
pub fn compare(a: &[u8], b: &[u8]) -> (usize, u8) {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .filter(|(x, y)| x != y)
        .fold((0, 0), |(n, m), (x, y)| (n + 1, m.max(x.abs_diff(*y))))
}

/// A closed polygon path.
pub fn polygon(points: &[Point]) -> Path {
    let mut path = PathBuilder::new();
    path.move_to(points[0]);
    for &p in &points[1..] {
        path.line_to(p).unwrap();
    }
    path.close();
    path.finish()
}

/// An axis-aligned rectangle path.
pub fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Path {
    let mut path = PathBuilder::new();
    path.rect(x, y, width, height);
    path.finish()
}
