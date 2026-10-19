//! Signed-area coverage of flattened paths. Curves become chords within a
//! device tolerance, and overlapping contours accumulate their winding before
//! the nonzero or even-odd rule clamps it; neither step is exact (ADR 0004).

use pagura_core::{Error, Limits, Result};
use pagura_raster::{FillRule, Path, Point, Segment, Transform};

/// Byte coverage at a device-pixel offset.
#[derive(Debug)]
pub(super) struct Coverage {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: usize,
    pub(super) data: Vec<u8>,
}

impl Coverage {
    pub(super) fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            data: Vec::new(),
        }
    }

    pub(super) fn height(&self) -> usize {
        self.data.len().checked_div(self.width).unwrap_or(0)
    }
}

/// Rasterizes `path` within the page `viewport`, which every tile shares, so
/// tiles assemble into exactly the page's pixels.
pub(super) fn rasterize(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: Limits,
    tolerance: f64,
    viewport: (u32, u32),
) -> Result<Coverage> {
    let (mut min, mut max) = bounds(path, transform, limits)?;
    min.x = min.x.max(0.0);
    min.y = min.y.max(0.0);
    max.x = max.x.min(f64::from(viewport.0));
    max.y = max.y.min(f64::from(viewport.1));
    let (x, y, width, height) = extent(min, max, limits)?;
    if width == 0 || height == 0 {
        return Ok(Coverage::empty());
    }
    let local = transform.then(Transform::translate(-f64::from(x), -f64::from(y)));
    let edges = flattened_edges(path, local, tolerance, &limits)?;
    let data = accumulate(&edges, width, height, rule, &limits)?;
    Ok(Coverage { x, y, width, data })
}

/// A path flattened once under a linear map, without its translation.
#[derive(Debug)]
pub(super) struct Geometry {
    pub(super) edges: Vec<(Point, Point)>,
    pub(super) min: Point,
    pub(super) max: Point,
}

impl Geometry {
    pub(super) fn new(
        path: &Path,
        transform: Transform,
        tolerance: f64,
        limits: Limits,
    ) -> Result<Self> {
        let transform = Transform {
            e: 0.0,
            f: 0.0,
            ..transform
        };
        let (min, max) = bounds(path, transform, limits)?;
        let flatten_limits = Limits {
            path_segments: limits
                .path_segments
                .min(limits.decoded_bytes / (2 * size_of::<(Point, Point)>())),
            ..limits
        };
        let edges = flattened_edges(path, transform, tolerance, &flatten_limits)?;
        edges
            .len()
            .checked_mul(size_of::<(Point, Point)>())
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("glyph geometry allocation budget exceeded"))?;
        Ok(Self { edges, min, max })
    }
}

fn bounds(path: &Path, transform: Transform, limits: Limits) -> Result<(Point, Point)> {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    if path.segments().len() > limits.path_segments || !transform.is_finite() {
        return Err(Error::limit("coverage geometry budget exceeded"));
    }
    for segment in path.segments() {
        let points: &[Point] = match segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => std::slice::from_ref(p),
            Segment::CubicTo(a, b, c) => &[*a, *b, *c],
            Segment::Close => &[],
        };
        for &point in points {
            let p = point * transform;
            if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e8 {
                return Err(Error::limit("coverage coordinate range exceeded"));
            }
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    Ok((min, max))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite device coordinates and pixel/allocation budgets bound extents before use"
)]
fn extent(min: Point, max: Point, limits: Limits) -> Result<(i32, i32, usize, usize)> {
    if min.x >= max.x || min.y >= max.y {
        return Ok((0, 0, 0, 0));
    }
    if !min.is_finite()
        || !max.is_finite()
        || min
            .x
            .abs()
            .max(min.y.abs())
            .max(max.x.abs())
            .max(max.y.abs())
            > 1e8
    {
        return Err(Error::limit("coverage coordinate range exceeded"));
    }
    let x = min.x.floor() as i32;
    let y = min.y.floor() as i32;
    let width = (max.x.ceil() - f64::from(x)) as usize;
    let height = (max.y.ceil() - f64::from(y)) as usize;
    let count = width
        .checked_mul(height)
        .filter(|&n| n <= limits.pixels && n <= limits.raster_work)
        .ok_or(Error::limit("coverage pixel budget exceeded"))?;
    // One band of f64 cells, with a sentinel per row, coexists with the mask.
    BAND.min(height)
        .checked_mul(width + 1)
        .and_then(|n| n.checked_mul(8))
        .and_then(|bytes| bytes.checked_add(count))
        .filter(|&bytes| bytes <= limits.decoded_bytes)
        .ok_or(Error::limit("coverage allocation budget exceeded"))?;
    Ok((x, y, width, height))
}

/// Device points snap to a 1/4096-pixel grid, so last-bit differences in
/// upstream geometry (stroke joins use platform `sin`, `cos` and `hypot`)
/// cannot move a coverage rounding boundary between platforms.
fn snap(p: Point) -> Point {
    const GRID: f64 = 4096.0;
    Point::new((p.x * GRID).round() / GRID, (p.y * GRID).round() / GRID)
}

fn flattened_edges(
    path: &Path,
    local: Transform,
    tolerance: f64,
    limits: &Limits,
) -> Result<Vec<(Point, Point)>> {
    let mut edges = Vec::new();
    let mut current = Point::default();
    let mut first = None;
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(start) = first {
                    edges.push((current, start));
                }
                current = snap(p * local);
                first = Some(current);
            }
            Segment::LineTo(p) => {
                let end = snap(p * local);
                edges.push((current, end));
                current = end;
            }
            Segment::CubicTo(a, b, c) => {
                flatten(
                    [current, snap(a * local), snap(b * local), snap(c * local)],
                    tolerance,
                    0,
                    &mut edges,
                    limits,
                )?;
                current = snap(c * local);
            }
            Segment::Close => {
                if let Some(start) = first {
                    edges.push((current, start));
                    current = start;
                }
            }
        }
        if edges.len() > limits.path_segments {
            return Err(Error::limit("coverage edge budget exceeded"));
        }
    }
    if let Some(start) = first {
        edges.push((current, start));
    }
    if edges.len() > limits.path_segments {
        return Err(Error::limit("coverage edge budget exceeded"));
    }
    Ok(edges)
}

/// Rows of signed area accumulated at once; rows are independent, so the
/// band height bounds storage without changing any value.
const BAND: usize = 256;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel indices are clamped to the checked surface dimensions and coverage to a byte"
)]
fn accumulate(
    edges: &[(Point, Point)],
    width: usize,
    height: usize,
    rule: FillRule,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let stride = width + 1;
    let mut data = Vec::with_capacity(width * height);
    let mut area = vec![0.0_f64; BAND.min(height) * stride];
    let mut work = 0_usize;
    for first in (0..height).step_by(BAND) {
        let last = (first + BAND).min(height);
        let band = &mut area[..(last - first) * stride];
        band.fill(0.0);
        work = work.saturating_add(edges.len());
        for &(mut a, mut b) in edges {
            let sign = if a.y < b.y {
                1.0
            } else {
                std::mem::swap(&mut a, &mut b);
                -1.0
            };
            if a.y == b.y {
                continue;
            }
            let slope = (b.x - a.x) / (b.y - a.y);
            let rows = (a.y.floor().max(0.0) as usize).max(first)..(b.y.ceil() as usize).min(last);
            for row in rows {
                let low = a.y.max(row as f64);
                let high = b.y.min((row + 1) as f64);
                let x0 = a.x + slope * (low - a.y);
                let x1 = a.x + slope * (high - a.y);
                let left = x0.min(x1).clamp(0.0, width as f64);
                let right = x0.max(x1).clamp(0.0, width as f64);
                let lo = left.floor() as usize;
                let hi = (right.ceil() as usize).min(width);
                work = work.saturating_add(hi - lo + 1);
                if work > limits.raster_work {
                    return Err(Error::limit("coverage work budget exceeded"));
                }
                let row = &mut band[(row - first) * stride..(row - first + 1) * stride];
                // A line within one column contributes a trapezoid.
                if hi == lo + 1
                    && x0 >= lo as f64
                    && x1 >= lo as f64
                    && x0 <= hi as f64
                    && x1 <= hi as f64
                {
                    let height = (high - low) * sign;
                    let value = (hi as f64 - x0.midpoint(x1)) * height;
                    row[lo] += value;
                    row[hi] += height - value;
                    continue;
                }
                let mut previous = 0.0;
                for (offset, cell) in row[lo..hi].iter_mut().enumerate() {
                    let boundary = (lo + offset + 1) as f64;
                    let value = if (x1 - x0).abs() < 1e-12 {
                        (boundary - x0).clamp(0.0, 1.0)
                    } else {
                        (primitive(boundary - x0) - primitive(boundary - x1)) / (x1 - x0)
                    } * (high - low)
                        * sign;
                    *cell += value - previous;
                    previous = value;
                }
                row[hi] += (high - low) * sign - previous;
            }
        }
        for row in band.chunks_exact(stride) {
            let mut sum = 0.0;
            for &delta in &row[..width] {
                sum += delta;
                let coverage = if rule == FillRule::EvenOdd {
                    1.0 - (sum.abs().rem_euclid(2.0) - 1.0).abs()
                } else {
                    sum.abs().min(1.0)
                };
                data.push((coverage * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    Ok(data)
}

/// The area to the left of a unit-slope ramp, integrated across one column.
pub(super) fn primitive(x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else if x < 1.0 {
        x * x * 0.5
    } else {
        x - 0.5
    }
}

fn flatten(
    p: [Point; 4],
    tolerance: f64,
    depth: usize,
    edges: &mut Vec<(Point, Point)>,
    limits: &Limits,
) -> Result<()> {
    if edges.len() >= limits.path_segments || depth > limits.depth.min(64) {
        return Err(Error::limit("coverage flattening budget exceeded"));
    }
    let [a, b, c, d] = p;
    let dx = d.x - a.x;
    let dy = d.y - a.y;
    let len = dx * dx + dy * dy;
    let distance = |p: Point| {
        let cross = (p.x - a.x) * dy - (p.y - a.y) * dx;
        cross * cross
    };
    let flat = if len < 1e-20 {
        [b, c]
            .iter()
            .all(|p| (p.x - a.x).abs().max((p.y - a.y).abs()) <= tolerance)
    } else {
        distance(b).max(distance(c)) <= tolerance * tolerance * len
            && [b, c].iter().all(|p| {
                let t = (p.x - a.x) * dx + (p.y - a.y) * dy;
                t >= 0.0 && t <= len
            })
    };
    if flat {
        edges.push((a, d));
        return Ok(());
    }
    let mid = |a: Point, b: Point| Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y));
    let ab = mid(a, b);
    let bc = mid(b, c);
    let cd = mid(c, d);
    let abc = mid(ab, bc);
    let bcd = mid(bc, cd);
    let m = mid(abc, bcd);
    flatten([a, ab, abc, m], tolerance, depth + 1, edges, limits)?;
    flatten([m, bcd, cd, d], tolerance, depth + 1, edges, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::PathBuilder;

    #[test]
    fn rows_across_bands_have_independent_rectangle_areas() {
        let mut p = PathBuilder::new();
        p.rect(0.25, 0.5, 3.5, 700.0);
        let mask = rasterize(
            &p.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits::default(),
            0.25,
            (8, 1000),
        )
        .unwrap();
        assert_eq!((mask.width, mask.height()), (4, 701));
        // Column areas are 3/4, 1, 1 and 3/4; the first and last rows are halves.
        for (row, values) in mask.data.as_chunks::<4>().0.iter().enumerate() {
            let expected = if row == 0 || row == 700 {
                [96, 128, 128, 96]
            } else {
                [191, 255, 255, 191]
            };
            assert_eq!(*values, expected, "row {row}");
        }
    }

    #[test]
    fn rectangles_and_triangle_have_independent_analytic_areas() {
        let mut p = PathBuilder::new();
        p.rect(0.25, 0.25, 1.0, 1.0);
        let mask = rasterize(
            &p.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits::default(),
            0.25,
            (8, 8),
        )
        .unwrap();
        assert_eq!(mask.data, [143, 48, 48, 16]);
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(1.0, 0.0));
        p.line_to(Point::new(0.0, 1.0));
        p.close();
        let mask = rasterize(
            &p.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits::default(),
            0.25,
            (8, 8),
        )
        .unwrap();
        assert_eq!(mask.data, [128]);
    }

    #[test]
    fn rejects_cells_work_geometry_and_storage_before_allocating() {
        let mut p = PathBuilder::new();
        p.rect(0.0, 0.0, 2.0, 2.0);
        let p = p.finish();
        for limits in [
            Limits {
                pixels: 3,
                ..Limits::default()
            },
            Limits {
                decoded_bytes: 50,
                ..Limits::default()
            },
            Limits {
                raster_work: 3,
                ..Limits::default()
            },
            Limits {
                path_segments: 2,
                ..Limits::default()
            },
        ] {
            assert!(
                rasterize(
                    &p,
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    limits,
                    0.25,
                    (8, 8)
                )
                .is_err()
            );
        }
        assert!(
            rasterize(
                &p,
                Transform::scale(1e20, 1e20),
                FillRule::NonZero,
                Limits::default(),
                0.25,
                (8, 8)
            )
            .is_err()
        );
    }

    #[test]
    fn subpixel_trapezoids_match_integer_area_in_both_directions() {
        for bottom in 0..=8_u32 {
            for top in 0..=8_u32 {
                for height in 1..=8_u32 {
                    let a = Point::new(f64::from(bottom) / 8.0, 0.0);
                    let b = Point::new(f64::from(top) / 8.0, f64::from(height) / 8.0);
                    let c = Point::new(1.0, b.y);
                    let d = Point::new(1.0, 0.0);
                    // The two parallel widths are (8-bottom)/8 and (8-top)/8.
                    let numerator = (16 - bottom - top) * height * 255;
                    let expected = u8::try_from((numerator + 64) / 128).unwrap();
                    for edges in [[(a, b), (c, d)], [(b, a), (d, c)]] {
                        assert_eq!(
                            accumulate(&edges, 1, 1, FillRule::NonZero, &Limits::default())
                                .unwrap(),
                            [expected]
                        );
                    }
                }
            }
        }
    }
}
