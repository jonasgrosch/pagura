//! A sufficient opaque constant-cell certificate; never a geometric-union
//! approximation of independently antialiased overlapping marks.

mod interval;

use interval::Interval;

use crate::prepare::{Budget, index};
use pagura_color::ColorSpace;
use pagura_core::{Limits, Rect, Result};
use pagura_interpret::{FillNode, GroupRole, Node, Paint, Scene, Tiling, TilingColors};
use pagura_raster::{DeviceRect, Path, Point, Segment, Transform};

/// Inspect only a single opaque DeviceGray-black rectangle and rectangular
/// local clips. Stored cell paths already include their local CTM. Return
/// the original fill only when every device pixel has an entirely interior
/// cell instance; all other cells retain the ordinary instance limit.
pub(super) fn fill<'a>(
    scene: &'a Scene,
    tiling: &Tiling,
    transform: Transform,
    indices: ((i64, i64), (i64, i64)),
    bounds: DeviceRect,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Option<&'a FillNode>> {
    budget.work.charge(1)?;
    let Some(cell) = scene.group(tiling.cell) else {
        return Ok(None);
    };
    if tiling.colors != TilingColors::Colored || cell.role != GroupRole::Tiling {
        return Ok(None);
    }
    let [id] = cell.children.as_slice() else {
        return Ok(None);
    };
    let Some(Node::Fill(node)) = scene.node(*id) else {
        return Ok(None);
    };
    if !opaque_black(scene, node) {
        return Ok(None);
    }
    budget.work.charge(2)?;
    if !Point::new(tiling.bbox.x0(), tiling.bbox.y0()).within_working_range()
        || !Point::new(tiling.bbox.x1(), tiling.bbox.y1()).within_working_range()
    {
        return Ok(None);
    }
    let Some(ink) = rectangle(&node.path, budget)? else {
        return Ok(None);
    };
    let Some(mut inside) = ink.intersection(&tiling.bbox) else {
        return Ok(None);
    };
    let mut magnitude = maximum(ink, maximum(tiling.bbox, [0.0; 2]));
    let mut current = node.mark.clip;
    let mut count = 0;
    while let Some(id) = current {
        if count >= limits.mask_sources {
            return Ok(None);
        }
        count += 1;
        budget.work.charge(1)?;
        let Some(clip) = scene.clips.get(index(id.0)) else {
            return Ok(None);
        };
        if clip.parent.is_some_and(|parent| parent.0 >= id.0) {
            return Ok(None);
        }
        let Some(rectangle) = rectangle(&clip.path, budget)? else {
            return Ok(None);
        };
        let Some(intersection) = inside.intersection(&rectangle) else {
            return Ok(None);
        };
        inside = intersection;
        magnitude = maximum(rectangle, magnitude);
        current = clip.parent;
    }
    budget.work.charge(512)?;
    Ok(certified(inside, magnitude, tiling.step, transform, bounds, indices).then_some(node))
}

fn opaque_black(scene: &Scene, node: &FillNode) -> bool {
    let Some(state) = scene.states.get(node.mark.state.index()) else {
        return false;
    };
    if !state.is_plain()
        || state.alpha_is_shape
        || state.overprint.enabled
        || state.conversion.black_generation.is_some()
        || state.conversion.undercolor_removal.is_some()
    {
        return false;
    }
    let Some(paint) = scene.paints.get(node.paint.index()) else {
        return false;
    };
    let Paint::Solid(color) = &**paint else {
        return false;
    };
    matches!(*color.space, ColorSpace::DeviceGray) && *color.components == [0.0]
}

fn rectangle(path: &Path, budget: &mut Budget) -> Result<Option<Rect>> {
    budget.work.charge(1)?;
    if path.segments().len() != 5 {
        return Ok(None);
    }
    budget.work.charge(9)?;
    let [
        Segment::MoveTo(a),
        Segment::LineTo(b),
        Segment::LineTo(c),
        Segment::LineTo(d),
        Segment::Close,
    ] = path.segments()
    else {
        return Ok(None);
    };
    if ![a, b, c, d]
        .into_iter()
        .all(|point| point.within_working_range())
    {
        return Ok(None);
    }
    let horizontal = a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x;
    let vertical = a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y;
    if !horizontal && !vertical {
        return Ok(None);
    }
    Ok(Rect::from_corners(a.x, a.y, c.x, c.y).filter(|rect| !rect.is_empty()))
}

fn maximum(rect: Rect, prior: [f64; 2]) -> [f64; 2] {
    [
        prior[0].max(rect.x0().abs()).max(rect.x1().abs()),
        prior[1].max(rect.y0().abs()).max(rect.y1().abs()),
    ]
}

/// Every operand of these outward upper-bound operations is nonnegative.
fn add(a: f64, b: f64) -> f64 {
    (a + b).next_up()
}
fn mul(a: f64, b: f64) -> f64 {
    (a * b).next_up()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "ordinary instance range validated indices below 4e15, hence exactly representable"
)]
fn certified(
    rect: Rect,
    points: [f64; 2],
    step: [f64; 2],
    t: Transform,
    bounds: DeviceRect,
    ((i0, i1), (j0, j1)): ((i64, i64), (i64, i64)),
) -> bool {
    let Some((det, span)) = inverse_span(t) else {
        return false;
    };
    let offsets = [
        mul((i0 as f64).abs().max((i1 as f64).abs()), step[0].abs()),
        mul((j0 as f64).abs().max((j1 as f64).abs()), step[1].abs()),
    ];
    let extent = [add(offsets[0], points[0]), add(offsets[1], points[1])];
    let absolute = [
        add(
            add(mul(t.a.abs(), extent[0]), mul(t.c.abs(), extent[1])),
            t.e.abs(),
        ),
        add(
            add(mul(t.b.abs(), extent[0]), mul(t.d.abs(), extent[1])),
            t.f.abs(),
        ),
    ];
    // i*step, offset.then(t), and then applying that map to a stored vertex
    // have fewer than 16 elementary floating operations per dependency path.
    // Standard gamma_n error uses absolute term sums, including cancellation;
    // EPSILON rather than EPSILON/2 and 16 MIN_POSITIVE also cover underflow.
    let n = 16.0 * f64::EPSILON;
    let gamma = (n / (1.0 - n).next_down()).next_up();
    let error = add(
        mul(gamma, absolute[0].max(absolute[1])),
        16.0 * f64::MIN_POSITIVE,
    );
    // A strict eligibility domain, not a coverage tolerance or changed limit.
    // It bounds absolute mapped terms below 2^32, inside the ordinary raster
    // working range. High/uncertain arithmetic falls back to the old refusal.
    if !error.is_finite() || error >= 1.0 / 65536.0 {
        return false;
    }
    let radius = [mul(1.5, span[0]), mul(1.5, span[1])];
    let erosion = [mul(span[0], error), mul(span[1], error)];
    let required = |axis: usize| add(step[axis].abs(), mul(2.0, add(radius[axis], erosion[axis])));
    // An inverse 3x3 device-pixel hull fits inside some Cartesian translation
    // whenever its span plus one lattice period is strictly smaller than R.
    // Shrinking both sides by the map-error interval leaves an entire device
    // pixel guard on every side: one cell gives byte 255 regardless of every
    // other independently antialiased cell's source-over contribution.
    let width = (rect.x1() - rect.x0()).next_down();
    let height = (rect.y1() - rect.y0()).next_down();
    if !(width.is_finite() && height.is_finite() && width > required(0) && height > required(1)) {
        return false;
    }
    let anchors = (|| {
        Some([
            Interval::point(rect.x0())?
                .add(Interval::point(radius[0])?)?
                .add(Interval::point(erosion[0])?)?,
            Interval::point(rect.y0())?
                .add(Interval::point(radius[1])?)?
                .add(Interval::point(erosion[1])?)?,
        ])
    })();
    anchors.is_some_and(|anchors| {
        witnesses(t, bounds, det, anchors, step, ((i0, i1), (j0, j1))).is_some()
    })
}

/// Enclose the determinant of the exact stored transform and inverse row
/// norms, instead of treating the rounded inverse as exact.
fn inverse_span(t: Transform) -> Option<(Interval, [f64; 2])> {
    let det = Interval::point(t.a)?
        .mul(Interval::point(t.d)?)?
        .sub(Interval::point(t.b)?.mul(Interval::point(t.c)?)?)?;
    let minimum = if det.lower() > 0.0 {
        det.lower()
    } else if det.upper() < 0.0 {
        -det.upper()
    } else {
        return None;
    };
    let span = [
        (add(t.d.abs(), t.c.abs()) / minimum).next_up(),
        (add(t.b.abs(), t.a.abs()) / minimum).next_up(),
    ];
    span.iter().all(|x| x.is_finite()).then_some((det, span))
}

/// Select floor witnesses from an independent outward inverse-center hull.
/// The whole witness interval must fit the actual enumerator's finite range;
/// only then does the forward-placement error bound cover every chosen cell.
#[expect(
    clippy::cast_precision_loss,
    reason = "ordinary instance range validated indices below 4e15"
)]
fn witnesses(
    t: Transform,
    bounds: DeviceRect,
    det: Interval,
    anchors: [Interval; 2],
    step: [f64; 2],
    ((i0, i1), (j0, j1)): ((i64, i64), (i64, i64)),
) -> Option<()> {
    if bounds.is_empty() {
        return None;
    }
    let x = Interval::new(f64::from(bounds.x0) + 0.5, f64::from(bounds.x1) - 0.5)?
        .sub(Interval::point(t.e)?)?;
    let y = Interval::new(f64::from(bounds.y0) + 0.5, f64::from(bounds.y1) - 0.5)?
        .sub(Interval::point(t.f)?)?;
    let centers = [
        Interval::point(t.d)?
            .mul(x)?
            .sub(Interval::point(t.c)?.mul(y)?)?
            .div(det)?,
        Interval::point(t.a)?
            .mul(y)?
            .sub(Interval::point(t.b)?.mul(x)?)?
            .div(det)?,
    ];
    for (axis, (lo, hi)) in [(i0, i1), (j0, j1)].into_iter().enumerate() {
        let range = centers[axis]
            .sub(anchors[axis])?
            .div(Interval::point(step[axis].abs())?)?;
        let (low, high) = (range.lower().floor(), range.upper().floor());
        let (low, high) = if step[axis] < 0.0 {
            (-high, -low)
        } else {
            (low, high)
        };
        if low < lo as f64 || high > hi as f64 {
            return None;
        }
    }
    Some(())
}
