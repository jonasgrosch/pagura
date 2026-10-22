//! Coverage-byte certification at the preparation boundary. A small area
//! error alone cannot survive repeated composition: both ends of the true
//! coverage interval must quantize to the byte execution will emit.

use super::{GlyphEdges, Kind, flattened_with, translated};
use crate::band::cells::Cells;
use crate::band::clip::Paths;
use crate::band::edges::{self, Bounds, Line};
use crate::band::shape::{Body, Frame, Shape, Winding};
use crate::band::union::{Meters, Piece, Resolver};
use crate::band::{Prep, RASTER_WORK, RENDER_BYTES, curve, raster};
use crate::flatten::{Hull, Inspection};
use crate::{FillRule, Path, Point, Segment, Transform, quantize_unit};
use pagura_core::{Error, Meter, Result};

const AREA: f64 = 1.0 / 4096.0;
const ROUNDS: i32 = 12;

#[derive(Debug, Clone, Copy)]
struct Pixel {
    x: u32,
    y: u32,
    byte: u8,
}

#[derive(Clone, Copy)]
struct Source<'a> {
    outline: &'a Path,
    linear: Transform,
    origin: Point,
    points: &'a [Point],
}

impl<'a> Source<'a> {
    fn of(edges: &'a GlyphEdges, origin: Point) -> Option<Self> {
        let (outline, linear, points) = match &edges.kind {
            Kind::Curve {
                outline, linear, ..
            } => (outline.as_ref(), *linear, &[][..]),
            Kind::Polygon {
                source: Some((outline, linear)),
                points,
                ..
            } => (outline.as_ref(), *linear, points.as_slice()),
            Kind::Polygon { source: None, .. } => return None,
        };
        outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
            .then_some(Self {
                outline,
                linear,
                origin,
                points,
            })
    }
}

/// A placement leaves this seam in the ordinary shape representation;
/// there are no execution-time corrections or fallible raster operations.
pub(super) fn certify(
    edges: &GlyphEdges,
    origin: Point,
    mut shape: Shape,
    options: &Prep<'_>,
) -> Result<Shape> {
    let Some(source) = Source::of(edges, origin) else {
        return Ok(shape);
    };
    let Source {
        outline, linear, ..
    } = source;
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes())?;
    let mut targets = Vec::new();
    for round in 0..=ROUNDS {
        let analytic = match &shape.body {
            Body::Glyph(_) => true,
            Body::Edges(body) => !body.curves.is_empty(),
        };
        work.charge(outline.segments().len())?;
        let error = if analytic {
            curve::reduction_area(outline, linear)
        } else {
            AREA
        } + placement_error(outline, linear, origin, &shape);
        let pending = ambiguous(&shape, error, (&mut work, &mut bytes))?;
        let mut wrong = false;
        for pixel in pending {
            let expected = true_byte(source, pixel, options, (&mut work, &mut bytes))?;
            if expected != pixel.byte {
                wrong = true;
                let target = [
                    f64::from(pixel.x),
                    f64::from(pixel.y),
                    f64::from(pixel.x) + 1.0,
                    f64::from(pixel.y) + 1.0,
                ];
                work.charge(targets.len())?;
                if !targets.contains(&target) {
                    bytes.reserve_capacity(&mut targets, 1)?;
                    targets.push(target);
                }
            }
        }
        let target_bytes = targets.capacity() * size_of::<[f64; 4]>();
        // Inspection cells, pending pixels and local interval geometry have
        // left scope. Keep the target allocation and the live shape charged.
        bytes.refund(
            bytes
                .used()
                .saturating_sub(shape.retained_bytes() + target_bytes),
        );
        if !wrong {
            return Ok(shape);
        }
        if round == ROUNDS {
            break;
        }
        let before = bytes.used();
        let (polygon, points) = flattened_with(
            outline,
            linear,
            options.limits,
            &mut bytes,
            Some(Inspection {
                hulls: None,
                work: &mut work,
                rounding: 0.0,
                targets: &targets,
                offset: origin,
                factor: 16.0_f64.powi(-round - 1),
            }),
        )?;
        let polygon_bytes = polygon.heap_bytes() + points.capacity() * size_of::<Point>();
        bytes.refund(bytes.used().saturating_sub(before + polygon_bytes));
        // Old geometry stays charged until its replacement is fully ready.
        let prepared = translated(
            &polygon,
            &points,
            origin,
            Winding::Unknown,
            options,
            &mut bytes,
            Some(&mut work),
        )?
        .ok_or(Error::limit("unresolved curve coverage interval"))?;
        drop((polygon, points));
        bytes.refund(polygon_bytes);
        let released = shape.retained_bytes();
        drop(shape);
        bytes.refund(released);
        shape = prepared;
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

/// Translation and row/intersection arithmetic can move a boundary. The
/// per-edge homotopy bound is at most three times the two coordinate
/// displacements inside any pixel. This broad initial bound only selects
/// pixels for the local, original-control certificate below.
fn placement_error(outline: &Path, linear: Transform, origin: Point, shape: &Shape) -> f64 {
    let mut magnitude = origin.x.abs().max(origin.y.abs()).max(1.0);
    for segment in outline.segments() {
        let mut include = |p: Point| {
            magnitude = magnitude
                .max((p.x * linear.a).abs() + (p.y * linear.c).abs() + origin.x.abs())
                .max((p.x * linear.b).abs() + (p.y * linear.d).abs() + origin.y.abs());
        };
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => include(p),
            Segment::CubicTo { c1, c2, end } => {
                include(c1);
                include(c2);
                include(end);
            }
            Segment::Close => {}
        }
    }
    let pieces = f64::from(
        u32::try_from(outline.segments().len().max(shape.boundaries())).unwrap_or(u32::MAX),
    );
    384.0 * f64::EPSILON * magnitude * pieces
}

fn ambiguous(
    shape: &Shape,
    error: f64,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<Vec<Pixel>> {
    let width = match &shape.body {
        Body::Glyph(body) => body.scan.1.saturating_sub(body.scan.0),
        Body::Edges(body) => body.scan.1.saturating_sub(body.scan.0),
    };
    let mut cells = Cells::new(width, bytes)?;
    let cell_bytes = cells.retained_bytes();
    work.charge(shape.work())?;
    let mut pending = Vec::new();
    let mut failure = None;
    raster::inspect(shape, &mut cells, |y, x0, x1, area| {
        if failure.is_some() || quantize_unit(area - error) == quantize_unit(area + error) {
            return;
        }
        let result = (|| {
            work.charge(x1 - x0)?;
            bytes.reserve_capacity(&mut pending, x1 - x0)?;
            for x in x0..x1 {
                pending.push(Pixel {
                    x: u32::try_from(x).map_err(|_| Error::limit("glyph column exceeds u32"))?,
                    y,
                    byte: quantize_unit(area),
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            failure = Some(error);
        }
    })?;
    drop(cells);
    bytes.refund(cell_bytes);
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(pending)
}

fn true_byte(
    source: Source<'_>,
    pixel: Pixel,
    options: &Prep<'_>,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<u8> {
    for round in 0..=ROUNDS {
        let before = bytes.used();
        let result = interval(source, pixel, (options, round), (work, bytes));
        // All interval geometry and resolver scratch has left scope.
        bytes.refund(bytes.used().saturating_sub(before));
        let (area, error) = result?;
        let lo = quantize_unit(area - error);
        if lo == quantize_unit(area + error) {
            return Ok(lo);
        }
    }
    Err(Error::limit("unresolved curve coverage interval"))
}

fn interval(
    source: Source<'_>,
    pixel: Pixel,
    (options, round): (&Prep<'_>, i32),
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<(f64, f64)> {
    let Source {
        outline,
        linear,
        origin,
        points: source_points,
    } = source;
    let (ox, dx) = subtract(origin.x, f64::from(pixel.x));
    let (oy, dy) = subtract(origin.y, f64::from(pixel.y));
    let local = Transform {
        e: ox,
        f: oy,
        ..linear
    };
    let mut hulls = Vec::new();
    let frame = Frame::new(options).ok_or(Error::limit("unresolved curve coverage interval"))?;
    let bounds = clip_bounds(&frame, pixel);
    let targets = [bounds];
    let before = bytes.used();
    let (mut polygon, local_points) = flattened_with(
        outline,
        local,
        options.limits,
        bytes,
        Some(Inspection {
            hulls: Some(&mut hulls),
            work,
            rounding: dx.abs().max(dy.abs()) * (1.0 + 8.0 * f64::EPSILON),
            targets: &targets,
            offset: Point::default(),
            factor: 16.0_f64.powi(-round),
        }),
    )?;
    let retained = polygon.heap_bytes()
        + local_points.capacity() * size_of::<Point>()
        + hulls.capacity() * size_of::<Hull>();
    bytes.refund(bytes.used().saturating_sub(before + retained));
    let released = local_points.capacity() * size_of::<Point>();
    drop(local_points);
    bytes.refund(released);
    for &point in source_points {
        // A genuine point's floor belongs to its canonical placed position.
        let point = Point::new(
            (point.x + origin.x).floor() - f64::from(pixel.x),
            (point.y + origin.y).floor() - f64::from(pixel.y),
        );
        polygon.cell(point, options.limits, bytes)?;
    }
    let lines = edges::lines(&polygon, bytes)?;
    let released = polygon.heap_bytes();
    drop(polygon);
    bytes.refund(released);
    let mut pieces = Vec::new();
    bytes.reserve(&mut pieces, lines.len())?;
    pieces.extend(lines.iter().filter_map(|&line| Piece::new(line, 0.0, 0)));
    work.charge(lines.len())?;
    let released = lines.capacity() * size_of::<Line>();
    drop(lines);
    bytes.refund(released);
    let mut error = hulls.iter().fold(0.0, |sum, hull| {
        if hull.intersects(bounds) {
            outward(sum + hull.area)
        } else {
            sum
        }
    });
    let released = hulls.capacity() * size_of::<Hull>();
    drop(hulls);
    bytes.refund(released);
    error = outward(error + source_lines(outline, linear, origin, pixel, local, bounds, work)?);
    let mut rules = Vec::new();
    bytes.reserve_capacity(&mut rules, 1)?;
    rules.push(FillRule::NonZero);
    error = outward(error + clip_pieces(&frame, pixel, (&mut pieces, &mut rules), bytes)?);
    work.charge(pieces.len())?;
    let mut sides = Vec::new();
    Resolver::default().row(&pieces, &rules, 0.0, (&mut sides, Meters { work, bytes }))?;
    let mut cells = Cells::new(1, bytes)?;
    if !cells.open((0, 1), 0..1) {
        return Err(Error::limit("curve interval cells exceed allocation"));
    }
    work.charge_product(sides.len(), 8)?;
    for side in sides {
        cells.line(side);
    }
    let mut area = 0.0;
    cells.inspect(0, |_, _, value| area = value.abs());
    Ok((area, error))
}

fn clip_pieces(
    frame: &Frame<'_>,
    pixel: Pixel,
    (pieces, rules): (&mut Vec<Piece>, &mut Vec<FillRule>),
    bytes: &mut Meter,
) -> Result<f64> {
    let mut error = 0.0;
    if let Some(rect) = frame.rect {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        bytes.reserve_capacity(rules, 1)?;
        rules.push(FillRule::NonZero);
        let corners = [
            Point::new(rect.left, rect.top),
            Point::new(rect.right, rect.top),
            Point::new(rect.right, rect.bottom),
            Point::new(rect.left, rect.bottom),
        ];
        for i in 0..4 {
            let (line, bound) = localized_line(
                Line {
                    a: corners[i],
                    b: corners[(i + 1) % 4],
                },
                pixel,
            );
            error = outward(error + edge_error(line, bound, [0.0, 0.0, 1.0, 1.0]));
            if let Some(piece) = Piece::new(line, 0.0, group) {
                bytes.reserve_capacity(pieces, 1)?;
                pieces.push(piece);
            }
        }
    }
    if let Some(paths) = frame.paths {
        let group =
            u8::try_from(rules.len()).map_err(|_| Error::limit("clip path count exceeds u8"))?;
        let start = pieces.len();
        paths.pieces(pixel.y, group, (pieces, bytes))?;
        for piece in pieces.get_mut(start..).unwrap_or_default() {
            let (line, bound) = localized_line(piece.line, pixel);
            error = outward(error + edge_error(line, bound, [0.0, 0.0, 1.0, 1.0]));
            *piece = Piece::new(line, 0.0, piece.group)
                .ok_or(Error::limit("unresolved clip localization interval"))?;
        }
        let count = paths.rules().count();
        bytes.reserve_capacity(rules, count)?;
        rules.extend(paths.rules());
    }
    Ok(error)
}

/// Error-free two-sum subtraction, retaining the exact stored-coordinate
/// difference even when the integer pixel lies far from an endpoint.
fn subtract(a: f64, b: f64) -> (f64, f64) {
    let high = a - b;
    let back = high - a;
    (high, (a - (high - back)) + (-b - back))
}

fn outward(value: f64) -> f64 {
    value * (1.0 + 16.0 * f64::EPSILON)
}

fn localized_line(line: Line, pixel: Pixel) -> (Line, Point) {
    let shift = |p: Point| {
        let (x, dx) = subtract(p.x, f64::from(pixel.x));
        let (y, dy) = subtract(p.y, f64::from(pixel.y));
        (Point::new(x, y), Point::new(dx.abs(), dy.abs()))
    };
    let (a, da) = shift(line.a);
    let (b, db) = shift(line.b);
    (Line { a, b }, Point::new(da.x.max(db.x), da.y.max(db.y)))
}

fn edge_error(line: Line, delta: Point, bounds: [f64; 4]) -> f64 {
    let radius = delta.x.max(delta.y);
    let hull = Hull {
        bounds: [
            (line.a.x.min(line.b.x) - radius).next_down(),
            (line.a.y.min(line.b.y) - radius).next_down(),
            (line.a.x.max(line.b.x) + radius).next_up(),
            (line.a.y.max(line.b.y) + radius).next_up(),
        ],
        area: 0.0,
    };
    if !hull.intersects(bounds) {
        return 0.0;
    }
    // Each coordinate of the linear homotopy is monotone. Perturbation
    // adds at most two radii to its variation in a unit pixel; tangential
    // displacement of an axis-aligned edge sweeps no area.
    outward(
        4.0 * (delta.x * ((line.b.y - line.a.y).abs() + 2.0 * delta.y).min(1.0)
            + delta.y * ((line.b.x - line.a.x).abs() + 2.0 * delta.x).min(1.0))
            + 16.0 * delta.x * delta.y,
    )
}

fn clip_bounds(frame: &Frame<'_>, pixel: Pixel) -> [f64; 4] {
    let mut bounds = [0.0_f64, 0.0, 1.0, 1.0];
    let clips = frame
        .rect
        .into_iter()
        .chain(frame.paths.and_then(Paths::proof_bounds));
    for Bounds {
        left,
        top,
        right,
        bottom,
    } in clips
    {
        let local = |a, b, lower| {
            let (high, tail): (f64, f64) = subtract(a, b);
            if lower {
                (high - tail.abs()).next_down()
            } else {
                (high + tail.abs()).next_up()
            }
        };
        bounds[0] = bounds[0].max(local(left, f64::from(pixel.x), true));
        bounds[1] = bounds[1].max(local(top, f64::from(pixel.y), true));
        bounds[2] = bounds[2].min(local(right, f64::from(pixel.x), false));
        bounds[3] = bounds[3].min(local(bottom, f64::from(pixel.y), false));
    }
    bounds
}

fn source_lines(
    outline: &Path,
    linear: Transform,
    origin: Point,
    pixel: Pixel,
    local: Transform,
    bounds: [f64; 4],
    work: &mut Meter,
) -> Result<f64> {
    work.charge(outline.segments().len())?;
    let (mut current, mut start) = (None, Point::default());
    let mut error = 0.0;
    let line = |a: Point, b: Point| {
        let canonical = |p: Point| {
            let p = p * linear;
            Point::new(p.x + origin.x, p.y + origin.y)
        };
        let (target, tail) = localized_line(
            Line {
                a: canonical(a),
                b: canonical(b),
            },
            pixel,
        );
        let placed = Line {
            a: a * local,
            b: b * local,
        };
        let delta = Point::new(
            outward(
                (target.a.x - placed.a.x)
                    .abs()
                    .max((target.b.x - placed.b.x).abs())
                    + tail.x,
            ),
            outward(
                (target.a.y - placed.a.y)
                    .abs()
                    .max((target.b.y - placed.b.y).abs())
                    + tail.y,
            ),
        );
        edge_error(placed, delta, bounds)
    };
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, start));
                }
                current = Some(p);
                start = p;
            }
            Segment::LineTo(p) => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, p));
                }
                current = Some(p);
            }
            Segment::CubicTo { end, .. } => current = Some(end),
            Segment::Close => {
                if let Some(previous) = current {
                    error = outward(error + line(previous, start));
                }
                current = Some(start);
            }
        }
    }
    if let Some(previous) = current {
        error = outward(error + line(previous, start));
    }
    Ok(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Area, ClipShape, DeviceRect, PathBuilder, ShapeOptions};
    use pagura_core::Limits;

    #[test]
    fn canonical_clip_interpolation_cancellation_stays_inside_the_interval() {
        let limits = Limits::default();
        let n = 2.0_f64.powi(39);
        let u = 2.0_f64.powi(-13);
        let mut clip = PathBuilder::new();
        clip.move_to(Point::new(-n, -n));
        clip.line_to(Point::new(n, n + u)).unwrap();
        clip.line_to(Point::new(-n, n + 1.0)).unwrap();
        clip.close();
        let clip = clip.finish();
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        };
        let clip = ClipShape::new(
            &[(&clip, FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area,
                limits: &limits,
            },
        )
        .unwrap();
        // This quadratic mark contains the whole unit pixel and has its
        // curved boundary outside it. Only the canonical clip can change
        // the pixel, so zero curve/localization error is not sufficient.
        let mut mark = PathBuilder::new();
        mark.move_to(Point::new(-2.0, -2.0));
        mark.cubic_to(
            Point::new(-2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0 / 3.0, -10.0 / 3.0),
            Point::new(2.0, -2.0),
        )
        .unwrap();
        mark.line_to(Point::new(2.0, 2.0)).unwrap();
        mark.line_to(Point::new(-2.0, 2.0)).unwrap();
        mark.close();
        let mark = mark.finish();
        let source = Source {
            outline: &mark,
            linear: Transform::IDENTITY,
            origin: Point::default(),
            points: &[],
        };
        let options = Prep {
            area,
            limits: &limits,
            clip: Some(&clip),
        };
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let (actual, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        // The original stored line is x(y)=(y-u/2)/(1+u/(2N)). Integrate
        // from its x=0 crossing u/2 to y=1, independently of Line::x.
        let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
        assert_eq!(quantize_unit(exact), 127);
        assert!(
            (actual - exact).abs() <= error + 1e-15,
            "clip area{actual} ±{error} excludes analytic {exact}"
        );
    }
}
