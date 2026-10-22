//! Coverage through the band rasterizer for the crate's unit tests.

use crate::{
    Adjustment, Area, BandCoverage, BandRasterizer, Capacity, ClipShape, DeviceRect, FillRule,
    MarkCoverage, Path, Precision, Shape, ShapeOptions, Span, Stroke, Transform,
};
use pagura_core::{Limits, Meter, Result};

/// The coverage of `shape` within `clip` over `area`, row-major over its
/// region, and the work rasterizing it took. Checks that rows and spans come
/// in order within their bands and that filling the coverage never grows it.
pub(crate) fn raster(
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
    let region = area.region;
    let width = region.width() as usize;
    let mut out = vec![0; width * region.height() as usize];
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
            assert!(
                area.band_rows(b).contains(&row.y),
                "row {} outside band {b}",
                row.y
            );
            last_row = Some(row.y);
            let line = (row.y - region.y0) as usize * width;
            let mut last_end = 0;
            for span in row.spans() {
                let (x, values): (u32, Vec<u8>) = match span {
                    Span::Full { x, len } => (x, vec![255; len as usize]),
                    Span::Partial { x, coverage } => {
                        assert!(coverage.iter().all(|&v| v > 0 && v < 255), "{coverage:?}");
                        (x, coverage.to_vec())
                    }
                };
                assert!(x >= region.x0 && x as usize + values.len() <= region.x1 as usize);
                assert!(x >= last_end, "spans in order");
                last_end = x + u32::try_from(values.len()).unwrap();
                let start = line + (x - region.x0) as usize;
                out[start..start + values.len()].copy_from_slice(&values);
            }
        }
    }
    (out, raster.work_done())
}

/// Exact options for `region` in bands of 16 rows.
fn options(region: DeviceRect, limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area: Area {
            region,
            band_height: 16,
        },
        precision: Precision::Exact,
        limits,
    }
}

/// The exact coverage of a fill of `path` under `transform` and `rule` over
/// `region`, within the intersection of `clips` (device-space paths).
///
/// # Errors
///
/// Returns the errors of preparing the shape and the clip.
pub(crate) fn fill(
    region: DeviceRect,
    (path, transform, rule): (&Path, Transform, FillRule),
    clips: &[(&Path, FillRule)],
    limits: &Limits,
) -> Result<Vec<u8>> {
    let options = options(region, limits);
    let clip = (!clips.is_empty())
        .then(|| ClipShape::new(clips, Transform::IDENTITY, &options))
        .transpose()?;
    let shape = Shape::fill_with_clip(path, transform, rule, clip.as_ref(), &options)?;
    Ok(coverage(&options.area, shape.as_ref(), clip.as_ref()))
}

/// The exact coverage of a stroke of `path` (in the stroke's own space,
/// mapped by `transform`) over `region`.
///
/// # Errors
///
/// Returns the stroker's errors.
pub(crate) fn stroke(
    region: DeviceRect,
    path: &Path,
    stroke: &Stroke,
    transform: Transform,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let options = options(region, limits);
    let shape = Shape::stroke(
        path,
        stroke,
        Transform::IDENTITY,
        transform,
        Adjustment::None,
        &options,
    )?;
    Ok(coverage(&options.area, shape.as_ref(), None))
}

/// The coverage of the intersection of `clips` (device-space paths) over
/// `region`.
///
/// # Errors
///
/// Returns the errors of preparing the clip.
pub(crate) fn clip(
    region: DeviceRect,
    clips: &[(&Path, FillRule)],
    limits: &Limits,
) -> Result<Vec<u8>> {
    let options = options(region, limits);
    let clip = ClipShape::new(clips, Transform::IDENTITY, &options)?;
    Ok(raster(&options.area, None, Some(&clip)).0)
}

/// [`raster`]'s bytes, or nothing covered without a shape.
fn coverage(area: &Area, shape: Option<&Shape>, clip: Option<&ClipShape>) -> Vec<u8> {
    match shape {
        Some(shape) => raster(area, Some(shape), clip).0,
        None => vec![0; (area.region.width() * area.region.height()) as usize],
    }
}

/// The `width` × `height` region at the origin.
pub(crate) fn window(width: u32, height: u32) -> DeviceRect {
    DeviceRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    }
}
