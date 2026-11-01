//! Elide only clips whose entire rectangular chain strictly encloses a fill.
//! Cubic control hulls bound every point of the curve. Stroke, image and group
//! bounds are deliberately not inferred here. Mark order and opacity stay put.
use super::{Result, Scene};
use pagura_core::{Error, Meter, Rect};
use pagura_interpret::MarkInfo;
use pagura_raster::{Path, Point, Segment, Transform};

pub(super) fn chains(scene: &Scene, work: &mut Meter) -> Result<Vec<Option<Rect>>> {
    scene
        .clips
        .len()
        .checked_mul(std::mem::size_of::<Option<Rect>>())
        .filter(|&bytes| bytes <= super::super::MAX_FRAME)
        .ok_or(Error::limit("SVG clip proof byte budget"))?;
    let mut bounds: Vec<Option<Rect>> = Vec::new();
    bounds
        .try_reserve_exact(scene.clips.len())
        .map_err(|_| Error::limit("SVG clip proof allocation"))?;
    for (index, clip) in scene.clips.iter().enumerate() {
        work.charge(1)?;
        let own = rectangle(&clip.path);
        let certified = if let Some(parent) = clip.parent {
            let parent = parent.index();
            if parent >= index {
                return Err(Error::invalid_argument("SVG clip chain order").into());
            }
            own.zip(bounds[parent])
                .and_then(|(a, b)| a.intersection(&b))
        } else {
            own
        };
        bounds.push(certified);
    }
    Ok(bounds)
}

fn rectangle(path: &Path) -> Option<Rect> {
    let [
        Segment::MoveTo(a),
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        Segment::Close,
    ] = path.segments()
    else {
        return None;
    };
    if ![a, b, c, d].iter().all(|p| p.is_finite()) {
        return None;
    }
    let horizontal = a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x;
    let vertical = a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y;
    if !(horizontal || vertical) {
        return None;
    }
    Rect::from_corners(a.x.min(c.x), a.y.min(c.y), a.x.max(c.x), a.y.max(c.y))
        .filter(|r| r.width() > 0.0 && r.height() > 0.0)
}

pub(super) fn hull(path: &Path, work: &mut Meter) -> Result<Option<Rect>> {
    // Accumulate without allocating a second path or solving curve extrema.
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for segment in path.segments() {
        work.charge(1)?;
        let points = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => [Some(p), None, None],
            Segment::CubicTo { c1, c2, end } => [Some(c1), Some(c2), Some(end)],
            Segment::Close => continue,
        };
        for p in points.into_iter().flatten() {
            if !p.is_finite() {
                return Ok(None);
            }
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
        }
    }
    Ok(Rect::from_corners(lo.x, lo.y, hi.x, hi.y))
}

pub(super) fn transformed(bounds: Option<Rect>, t: Transform) -> Option<Rect> {
    let r = bounds?;
    if [t.a, t.b, t.c, t.d, t.e, t.f, r.x0(), r.y0(), r.x1(), r.y1()]
        .into_iter()
        .any(|v| !v.is_finite() || v.abs() > 1_000_000.0)
    {
        return None;
    }

    // Bounds also cover conversion to the browser's finite precision. Refuse
    // cancellation-heavy/extreme transforms rather than guessing a tight box.
    let magnitude = (t.a.abs() + t.b.abs()) * r.x0().abs().max(r.x1().abs())
        + (t.c.abs() + t.d.abs()) * r.y0().abs().max(r.y1().abs())
        + t.e.abs()
        + t.f.abs();
    if !magnitude.is_finite() || magnitude > 1_000_000.0 {
        return None;
    }
    let corners = [
        Point::new(r.x0(), r.y0()),
        Point::new(r.x1(), r.y0()),
        Point::new(r.x0(), r.y1()),
        Point::new(r.x1(), r.y1()),
    ]
    .map(|p| t.apply(p));
    let pad = 64.0 * f64::from(f32::EPSILON) * magnitude.max(1.0);
    Rect::from_corners(
        corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - pad,
        corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - pad,
        corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max)
            + pad,
        corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max)
            + pad,
    )
}

pub(super) fn inactive(
    mark: MarkInfo,
    bounds: Option<Rect>,
    chains: &[Option<Rect>],
    unit: f64,
) -> bool {
    let Some((bounds, clip)) = bounds.zip(mark.clip.and_then(|id| chains[id.index()])) else {
        return false;
    };
    // Eight SVG units at the document's natural size leave ample room for
    // antialiasing and float conversion; touching/near-edge bounds stay clipped.
    let magnitude = [
        bounds.x0(),
        bounds.y0(),
        bounds.x1(),
        bounds.y1(),
        clip.x0(),
        clip.y0(),
        clip.x1(),
        clip.y1(),
    ]
    .into_iter()
    .map(f64::abs)
    .fold(1.0, f64::max);
    let guard = 8.0 / unit + 64.0 * f64::from(f32::EPSILON) * magnitude;
    magnitude <= 1_000_000.0
        && guard.is_finite()
        && guard > 0.0
        && bounds.x0() > clip.x0() + guard
        && bounds.y0() > clip.y0() + guard
        && bounds.x1() < clip.x1() - guard
        && bounds.y1() < clip.y1() - guard
}
