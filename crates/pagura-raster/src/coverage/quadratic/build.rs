//! Curve preparation, degree reduction and viewport projection.

use super::{Piece, Quadratic, add, lerp, scale, sub};
use crate::coverage::{Edge, Geometry, RASTER_WORK, Rectangle, Row, Shape, clipped};
use crate::flatten::bounded;
use crate::{FillRule, Point, Segment};
use pagura_core::{Error, Limits, Meter, Result};

// A cubic is a degree-elevated quadratic exactly when its third difference
// D = -P0 + 3P1 - 3P2 + P3 vanishes. Its midpoint quadratic differs from it by
// D t(t - 1/2)(t - 1), at most |D| sqrt(3)/36, and rounding leaves D at a few
// ulps. Bounding each coordinate of D by 2^-24 pixels keeps that displacement
// below 2^-27: a strip of that width along a monotone arc, which is at most
// two pixels long inside any pixel. 32 pieces then change a pixel by under
// 2^-21 of its area, about 1e-4 levels. General cubics have no such bound, so
// their outlines keep certified polygons.
const REDUCIBLE: f64 = 1.0 / 16_777_216.0;

fn reducible(d: Point) -> bool {
    d.x.abs() <= REDUCIBLE && d.y.abs() <= REDUCIBLE
}

// Lines go through the line sweep's own viewport clipping; curved pieces are
// kept apart until both join the curve sweep.
struct Builder<'a> {
    bounds: Rectangle,
    limits: &'a Limits,
    row: &'a mut Row,
    lines: &'a mut Vec<Edge>,
    curves: &'a mut Vec<Piece>,
    group: usize,
    rule: FillRule,
    // Canonical preparation stops once admission can no longer succeed.
    cap: usize,
    general_cubic: bool,
}
impl Builder<'_> {
    fn pieces(&self) -> usize {
        self.lines.len().saturating_add(self.curves.len())
    }

    fn budget(&self) -> Result<()> {
        if self.pieces() >= self.limits.path_segments {
            return Err(Error::limit("quadratic edge budget exceeded"));
        }
        Ok(())
    }

    fn rejected(&self) -> bool {
        self.general_cubic || self.pieces() > self.cap
    }

    fn line(&mut self, a: Point, b: Point) -> Result<()> {
        self.row.charge(3)?;
        if self.bounds.is_empty() {
            return Ok(());
        }
        self.budget()?;
        clipped(
            self.lines,
            a,
            b,
            self.group,
            self.rule,
            [
                self.bounds.left,
                self.bounds.top,
                self.bounds.right,
                self.bounds.bottom,
            ],
            self.limits,
        )
    }

    fn cubic(&mut self, points: [Point; 4]) -> Result<()> {
        self.row.charge(1)?;
        let [a, b, c, d] = points;
        if !reducible(sub(sub(d, a), scale(sub(c, b), 3.0))) {
            self.general_cubic = true;
            return Ok(());
        }
        // Average the controls recovered from each degree-elevated cubic
        // control. This also interpolates the cubic at its midpoint.
        let control = add(lerp(a, d, 0.5), scale(add(sub(b, a), sub(c, d)), 0.75));
        self.quadratic(Quadratic {
            points: [a, control, d],
        })
    }

    fn quadratic(&mut self, curve: Quadratic) -> Result<()> {
        let [_, v, a] = curve.coefficients();
        let mut events = [0.0, 1.0, 0.0, 0.0];
        let mut count = 2;
        for (v, a) in [(v.x, a.x), (v.y, a.y)] {
            if a != 0.0 {
                let t = -v / (2.0 * a);
                if t > 0.0 && t < 1.0 {
                    events[count] = t;
                    count += 1;
                }
            }
        }
        let events = &mut events[..count];
        events.sort_unstable_by(f64::total_cmp);
        for pair in events.windows(2) {
            if pair[0] == pair[1] {
                continue;
            }
            let piece = curve.interval(pair[0], pair[1]);
            // A collinear quadratic's parameterization has no area effect.
            if super::cross(v, a) == 0.0 {
                self.line(piece.points[0], piece.points[2])?;
            } else {
                self.project(piece)?;
            }
        }
        Ok(())
    }

    fn project(&mut self, mut curve: Quadratic) -> Result<()> {
        if curve.points[0].y == curve.points[2].y || self.bounds.is_empty() {
            return Ok(());
        }
        let winding = if curve.points[0].y < curve.points[2].y {
            1
        } else {
            -1
        };
        if winding < 0 {
            curve.points.reverse();
        }
        let [first, _, last] = curve.points;
        if self.bounds.left <= first.x.min(last.x)
            && first.x.max(last.x) <= self.bounds.right
            && self.bounds.top <= first.y
            && last.y <= self.bounds.bottom
        {
            // A monotone piece within the viewport has no clipping events.
            return self.push(curve.interval(0.0, 1.0), winding);
        }
        let lo = curve.points[0].y.max(self.bounds.top);
        let hi = curve.points[2].y.min(self.bounds.bottom);
        if lo >= hi {
            return Ok(());
        }
        let mut events = [
            curve.parameter(lo, false),
            curve.parameter(hi, false),
            0.0,
            0.0,
        ];
        let mut count = 2;
        for x in [self.bounds.left, self.bounds.right] {
            let t = curve.parameter(x, true);
            if t > events[0] && t < events[1] {
                events[count] = t;
                count += 1;
            }
        }
        let events = &mut events[..count];
        events.sort_unstable_by(f64::total_cmp);
        for pair in events.windows(2) {
            self.push(curve.interval(pair[0], pair[1]), winding)?;
        }
        Ok(())
    }

    fn push(&mut self, piece: Quadratic, winding: i64) -> Result<()> {
        self.budget()?;
        self.row.charge(1)?;
        let [a, control, b] = piece.points;
        let edge = Edge {
            a,
            b,
            winding,
            group: self.group,
            rule: self.rule,
            id: 0,
        };
        let x = piece.at(0.5).x;
        // Outside the viewport only the winding matters, as for lines.
        if x <= self.bounds.left || x >= self.bounds.right {
            let x = x.clamp(self.bounds.left, self.bounds.right);
            self.lines.push(Edge {
                a: Point::new(x, a.y),
                b: Point::new(x, b.y),
                id: self.lines.len(),
                ..edge
            });
        } else {
            self.curves.push(Piece {
                edge,
                control: Some(control),
            });
        }
        Ok(())
    }

    fn finish(&mut self, start: Point, current: Point, degenerate: bool) -> Result<()> {
        if degenerate {
            let (x, y) = (start.x.floor(), start.y.floor());
            let points = [
                Point::new(x, y),
                Point::new(x + 1.0, y),
                Point::new(x + 1.0, y + 1.0),
                Point::new(x, y + 1.0),
            ];
            for (a, b) in points.into_iter().zip(points.into_iter().cycle().skip(1)) {
                self.line(a, b)?;
            }
            Ok(())
        } else {
            self.line(current, start)
        }
    }

    fn path(&mut self, path: &crate::Path, transform: crate::Transform) -> Result<()> {
        if path.segments().len() > self.limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        if !transform.is_finite() {
            return Err(Error::invalid_argument("path transform is not finite"));
        }
        let mut start = None;
        let mut current = None;
        let mut source_start = None;
        let mut source_current = None;
        let mut closed = false;
        let mut degenerate = true;
        let mut segments = 0;
        for segment in path.segments() {
            if self.rejected() {
                return Ok(());
            }
            self.row.charge(1)?;
            let map = |point: Point| -> Result<Point> {
                bounded(point)?;
                let point = point * transform;
                bounded(point)?;
                Ok(point)
            };
            match *segment {
                Segment::MoveTo(p) => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                    {
                        self.finish(a, b, degenerate)?;
                    }
                    source_start = Some(p);
                    source_current = Some(p);
                    let p = map(p)?;
                    start = Some(p);
                    current = Some(p);
                    degenerate = true;
                    closed = false;
                    segments = 0;
                }
                Segment::LineTo(p) => {
                    let source = p;
                    let p = map(p)?;
                    let a = current.ok_or(Error::invalid_argument("line has no current point"))?;
                    if closed {
                        start = Some(a);
                        source_start = source_current;
                        degenerate = true;
                        segments = 0;
                        closed = false;
                    }
                    degenerate &= source_current == Some(source);
                    source_current = Some(source);
                    self.line(a, p)?;
                    current = Some(p);
                    segments += 1;
                }
                Segment::CubicTo {
                    c1: b,
                    c2: c,
                    end: d,
                } => {
                    let a = current.ok_or(Error::invalid_argument("curve has no current point"))?;
                    if closed {
                        start = Some(a);
                        source_start = source_current;
                        degenerate = true;
                        segments = 0;
                        closed = false;
                    }
                    degenerate &= [b, c, d].iter().all(|p| Some(*p) == source_current);
                    source_current = Some(d);
                    let (b, c, d) = (map(b)?, map(c)?, map(d)?);
                    self.cubic([a, b, c, d])?;
                    current = Some(d);
                    segments += 1;
                }
                Segment::Close => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                    {
                        self.finish(a, b, degenerate)?;
                        current = Some(a);
                        source_current = source_start;
                        closed = true;
                    }
                }
            }
        }
        if let (Some(a), Some(b)) = (start, current)
            && !closed
            && segments > 0
        {
            self.finish(a, b, degenerate)?;
        }
        Ok(())
    }
}

/// Prepares monotone pieces in the translation-free device frame, or `None`
/// for a general cubic or once there are more than `cap` of them.
pub(crate) fn prepare(
    path: &crate::Path,
    transform: crate::Transform,
    limits: &Limits,
    cap: usize,
) -> Result<Option<Vec<Piece>>> {
    let mut row = Row {
        work: Meter::new(limits.raster_work, RASTER_WORK),
        maximum_events: limits.path_segments,
        ..Row::default()
    };
    let mut lines = Vec::new();
    let mut curves = Vec::new();
    let extent = 2.0_f64.powi(41);
    let mut builder = Builder {
        bounds: Rectangle {
            left: -extent,
            top: -extent,
            right: extent,
            bottom: extent,
        },
        limits,
        row: &mut row,
        lines: &mut lines,
        curves: &mut curves,
        group: 0,
        rule: FillRule::NonZero,
        cap,
        general_cubic: false,
    };
    builder.path(path, transform)?;
    if builder.rejected() {
        return Ok(None);
    }
    let mut pieces: Vec<_> = lines.into_iter().map(Piece::line).collect();
    pieces.append(&mut curves);
    Ok(Some(pieces))
}

/// Places the painted glyph's canonical pieces at their exact translation.
pub(super) fn glyph_edges(
    shapes: &[Shape],
    (bounds, anchor): (Rectangle, Point),
    limits: &Limits,
    row: &mut Row,
    (lines, curves): (&mut Vec<Edge>, &mut Vec<Piece>),
) -> Result<()> {
    let shape = &shapes[0];
    let Geometry::Glyph(glyph) = &shape.geometry else {
        return Err(Error::invalid_argument("missing canonical glyph"));
    };
    let pieces = glyph
        .curves()
        .ok_or(Error::invalid_argument("missing canonical curves"))?;
    row.charge(glyph.vertices())?;
    let offset = Point::new(shape.transform.e - anchor.x, shape.transform.f - anchor.y);
    let mut builder = Builder {
        bounds,
        limits,
        row,
        lines,
        curves,
        group: 0,
        rule: shape.rule,
        cap: usize::MAX,
        general_cubic: false,
    };
    for piece in pieces {
        let edge = piece.edge;
        if let Some(curve) = piece.quadratic() {
            let mut curve = Quadratic {
                points: curve.points.map(|p| add(p, offset)),
            };
            if edge.winding < 0 {
                curve.points.reverse();
            }
            builder.project(curve)?;
        } else {
            let (a, b) = if edge.winding < 0 {
                (edge.b, edge.a)
            } else {
                (edge.a, edge.b)
            };
            builder.line(add(a, offset), add(b, offset))?;
        }
    }
    Ok(())
}
