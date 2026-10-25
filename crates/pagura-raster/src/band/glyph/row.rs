//! An outline's sides and curves with the heights they reach, so that the
//! geometry of one pixel row visits only what can meet it.
//!
//! A polygon's winding within a row comes from its edges' parts in that
//! row alone. An edge, or a curve and every piece its flattening accepts,
//! that stays above or below the row has no part there, and no hull that
//! meets the row's pixels. A row's lines are therefore the flattened
//! outline's lines that can reach it, each built as the whole outline's
//! flattening builds it: a curve's pieces depend only on the curve, the map
//! and the refinement targets, and every vertex is mapped on its own.

use crate::band::edges::Line;
use crate::flatten::{self, CurveStack, Flatness, Inspection, bounded};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Limits, Meter, Result};

/// A straight side, a contour's closing side included, or a curve as cubic
/// controls with the rounding units its elevation adds, in outline space.
#[derive(Debug, Clone, Copy)]
enum Item {
    Line(Point, Point),
    Curve([Point; 4], f64),
}

/// An outline's items in path order, each with the box relative to the
/// glyph origin that its geometry can reach under the linear map.
#[derive(Debug)]
pub(super) struct Rows {
    items: Vec<(Item, [f64; 4])>,
}

/// How a row's lines are made: the map of their vertices, a translation
/// added after it, and the flattening and inspection of their curves.
pub(super) struct Making<'a> {
    pub(super) transform: Transform,
    pub(super) shift: Option<Point>,
    pub(super) flatness: Flatness,
    pub(super) inspection: Inspection<'a>,
}

impl Rows {
    /// The items of `outline` under `linear`. `slack` widens each item's
    /// box by more than the rounding of any frame its rows are built in, so
    /// that no item a row's geometry depends on is left out.
    pub(super) fn new(
        outline: &Path,
        linear: Transform,
        slack: f64,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let mut items = Vec::new();
        bytes.reserve(&mut items, outline.segments().len().saturating_add(1))?;
        let mut add = |item: Item| {
            let reach = |points: &[Point]| {
                let mut b = [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ];
                for &p in points {
                    let p = p * linear;
                    b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
                }
                [b[0] - slack, b[1] - slack, b[2] + slack, b[3] + slack]
            };
            let reach = match item {
                Item::Line(a, b) if a == b => return Ok(()),
                Item::Line(a, b) => reach(&[a, b]),
                Item::Curve(controls, _) => reach(&controls),
            };
            if items.len() == items.capacity() {
                bytes.reserve_capacity(&mut items, 1)?;
            }
            items.push((item, reach));
            Ok(())
        };
        // Every contour is closed, as the polygon closes it: at a move, at
        // a close and at the path's end, back to the subpath's start.
        let (mut start, mut current, mut open) = (Point::default(), Point::default(), false);
        for &segment in outline.segments() {
            match segment {
                Segment::MoveTo(p) => {
                    if open {
                        add(Item::Line(current, start))?;
                    }
                    (start, current, open) = (p, p, true);
                }
                Segment::LineTo(p) => {
                    add(Item::Line(current, p))?;
                    current = p;
                }
                Segment::Close => {
                    add(Item::Line(current, start))?;
                    current = start;
                }
                Segment::CubicTo { end, .. } | Segment::QuadTo { end, .. } => {
                    if let Some((controls, elevation)) = flatten::controls(current, segment) {
                        add(Item::Curve(controls, elevation))?;
                    }
                    current = end;
                }
            }
        }
        if open {
            add(Item::Line(current, start))?;
        }
        Ok(Self { items })
    }

    /// Calls `visit` with the ends of every straight side, horizontal ones
    /// included, that can reach relative heights `band`.
    pub(super) fn sides(
        &self,
        band: [f64; 2],
        work: &mut Meter,
        mut visit: impl FnMut(Point, Point),
    ) -> Result<()> {
        work.charge(self.items.len())?;
        for &(item, [_, lo, _, hi]) in &self.items {
            if let Item::Line(a, b) = item
                && hi > band[0]
                && lo < band[1]
            {
                visit(a, b);
            }
        }
        Ok(())
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.items.capacity() * size_of::<(Item, [f64; 4])>()
    }

    /// Appends to `out` the lines, not horizontal, of the items that can
    /// reach relative heights `band`, and their curves' hulls to the
    /// inspection's. `points` and `pending` are scratch.
    pub(super) fn lines(
        &self,
        band: [f64; 2],
        making: Making<'_>,
        limits: &Limits,
        (points, pending): (&mut Vec<Point>, &mut CurveStack),
        (out, bytes): (&mut Vec<Line>, &mut Meter),
    ) -> Result<()> {
        let Making {
            transform,
            shift,
            flatness,
            inspection:
                Inspection {
                    mut hulls,
                    work,
                    rounding,
                    targets,
                    offset,
                    factor,
                },
        } = making;
        let shift = shift.unwrap_or_default();
        let map = |p: Point| -> Result<Point> {
            let p = p * transform;
            let p = if shift == Point::default() {
                p
            } else {
                Point::new(p.x + shift.x, p.y + shift.y)
            };
            bounded(p)?;
            Ok(p)
        };
        let mut count = 0;
        for &(item, [_, lo, _, hi]) in &self.items {
            work.charge(1)?;
            if hi <= band[0] || lo >= band[1] {
                continue;
            }
            match item {
                Item::Line(a, b) => push(
                    Line {
                        a: map(a)?,
                        b: map(b)?,
                    },
                    out,
                    bytes,
                )?,
                Item::Curve(controls, elevation) => {
                    points.clear();
                    flatten::curve_inspected(
                        (controls, elevation),
                        transform,
                        limits,
                        (points, pending, &mut count),
                        (
                            flatness,
                            bytes,
                            Inspection {
                                hulls: hulls.as_deref_mut(),
                                work,
                                rounding,
                                targets,
                                offset,
                                factor,
                            },
                        ),
                    )?;
                    let mut a = map(controls[0])?;
                    for &p in points.iter() {
                        let b = map(p)?;
                        push(Line { a, b }, out, bytes)?;
                        a = b;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Keeps `line` unless it is horizontal: it bounds no area.
fn push(line: Line, out: &mut Vec<Line>, bytes: &mut Meter) -> Result<()> {
    if !line.is_horizontal() {
        if out.len() == out.capacity() {
            bytes.reserve_capacity(out, 1)?;
        }
        out.push(line);
    }
    Ok(())
}
