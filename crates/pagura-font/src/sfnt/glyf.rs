//! Unhinted TrueType contours, implied quadratic points, and components
//! (OpenType glyf).
use crate::budget::Work;
use crate::memory::vector;
use crate::read::{self, syntax};
use crate::{GlyphId, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};
use std::ops::Range;

/// Simple-glyph point flags (OpenType glyf). Bit 6 (overlap) is ignored.
mod point {
    pub(super) const ON_CURVE: u8 = 0x01;
    pub(super) const X_SHORT: u8 = 0x02;
    pub(super) const Y_SHORT: u8 = 0x04;
    pub(super) const REPEAT: u8 = 0x08;
    /// With the short flag, the sign; without it, "same as before".
    pub(super) const X_SAME_OR_POSITIVE: u8 = 0x10;
    pub(super) const Y_SAME_OR_POSITIVE: u8 = 0x20;
    pub(super) const RESERVED: u8 = 0x80;
}

/// Composite-glyph component flags (OpenType glyf).
mod component {
    pub(super) const ARGS_ARE_WORDS: u16 = 0x0001;
    pub(super) const ARGS_ARE_XY_VALUES: u16 = 0x0002;
    pub(super) const ROUND_XY_TO_GRID: u16 = 0x0004;
    pub(super) const HAVE_A_SCALE: u16 = 0x0008;
    pub(super) const MORE_COMPONENTS: u16 = 0x0020;
    pub(super) const HAVE_X_AND_Y_SCALE: u16 = 0x0040;
    pub(super) const HAVE_TWO_BY_TWO: u16 = 0x0080;
    pub(super) const HAVE_INSTRUCTIONS: u16 = 0x0100;
    pub(super) const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
    pub(super) const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;
    /// Every defined flag; bit 4 and bits 13-15 are reserved.
    pub(super) const DEFINED: u16 = 0x1fef;
}

/// numberOfContours of a composite glyph.
const COMPOSITE: i16 = -1;

/// The glyf table and the validated loca offsets into it.
#[derive(Debug)]
pub(super) struct Glyf {
    table: Range<usize>,
    /// `None` where a truncated loca table was repaired.
    locations: Vec<Option<usize>>,
}

#[derive(Clone, Copy)]
struct Vertex {
    point: Point,
    on: bool,
}

/// A glyph outline and its points, which components attach by number.
#[derive(Debug)]
struct Shape {
    path: Path,
    points: Vec<Point>,
}

struct Component {
    flags: u16,
    glyph: GlyphId,
    arguments: [i32; 2],
    transform: Transform,
}

impl Glyf {
    pub(super) const fn new(table: Range<usize>, locations: Vec<Option<usize>>) -> Self {
        Self { table, locations }
    }

    pub(super) fn heap_bytes(&self) -> usize {
        vector(&self.locations)
    }

    pub(super) fn outline(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
        Ok(self.load(data, glyph, &mut Vec::new(), work)?.path)
    }

    /// Loads `glyph`, whose composite ancestors are `parents`.
    fn load(
        &self,
        data: &[u8],
        glyph: GlyphId,
        parents: &mut Vec<GlyphId>,
        work: &mut Work<'_>,
    ) -> Result<Shape> {
        let table = Position::Buffer(self.table.start);
        if parents.contains(&glyph) {
            return Err(syntax(table, "recursive composite glyph"));
        }
        work.depth(parents.len(), usize::MAX, table)?;
        work.work(1, table)?;
        let index = u32_to_usize(glyph.0);
        let location = |i: usize| {
            self.locations
                .get(i)
                .copied()
                .flatten()
                .ok_or_else(|| syntax(table, "missing glyph location"))
        };
        let (start, end) = (location(index)?, location(index + 1)?);
        if start == end {
            return Ok(Shape {
                path: Path::default(),
                points: Vec::new(),
            });
        }
        if start > self.table.len() || end > self.table.len() {
            return Err(syntax(Position::Buffer(self.table.end), "truncated glyph"));
        }
        let base = self.table.start + start;
        let mut r = read::slice(data, base..self.table.start + end, "truncated glyph")?;
        let contours = r.i16()?;
        r.skip(8)?; // xMin, yMin, xMax, yMax
        match contours {
            0.. => simple(&mut r, usize::from(contours.unsigned_abs()), work),
            COMPOSITE => {
                parents.push(glyph);
                let shape = self.composite(data, &mut r, parents, work);
                parents.pop();
                shape
            }
            _ => Err(syntax(
                Position::Buffer(base),
                "invalid negative contour count",
            )),
        }
    }

    fn composite(
        &self,
        data: &[u8],
        r: &mut Cursor<'_>,
        parents: &mut Vec<GlyphId>,
        work: &mut Work<'_>,
    ) -> Result<Shape> {
        let mut points = Vec::new();
        let mut builder = PathBuilder::new();
        let mut have_instructions = false;
        loop {
            work.work(1, r.position())?;
            let Component {
                flags,
                glyph,
                arguments: [a, b],
                mut transform,
            } = component(r, work)?;
            let shape = self.load(data, glyph, parents, work)?;
            let at = r.position();
            let offset = if flags & component::ARGS_ARE_XY_VALUES != 0 {
                let p = Point::new(f64::from(a), f64::from(b));
                // OpenType leaves legacy offsets unspecified; use unscaled offsets
                // unless SCALED_COMPONENT_OFFSET explicitly requests scaling.
                if flags & component::SCALED_COMPONENT_OFFSET != 0 {
                    transform.apply(p)
                } else {
                    p
                }
            } else {
                let point = |points: &[Point], n: i32, missing| {
                    usize::try_from(n)
                        .ok()
                        .and_then(|n| points.get(n).copied())
                        .ok_or_else(|| syntax(at, missing))
                };
                let parent = point(&points, a, "missing parent attachment point")?;
                let child =
                    transform.apply(point(&shape.points, b, "missing child attachment point")?);
                Point::new(parent.x - child.x, parent.y - child.y)
            };
            let round = flags & component::ROUND_XY_TO_GRID != 0;
            transform.e = if round { offset.x.round() } else { offset.x };
            transform.f = if round { offset.y.round() } else { offset.y };
            work.work(shape.path.segments().len(), at)?;
            work.entries(shape.points.len(), at)?;
            work.segments(shape.path.segments().len(), at)?;
            for &p in &shape.points {
                let p = transform.apply(p);
                if !p.is_finite() {
                    return Err(syntax(at, "nonfinite component point"));
                }
                points.push(p);
            }
            append(&mut builder, &shape.path, transform, at)?;
            have_instructions |= flags & component::HAVE_INSTRUCTIONS != 0;
            if flags & component::MORE_COMPONENTS == 0 {
                break;
            }
        }
        if have_instructions {
            let len = usize::from(r.u16()?);
            r.skip(len)?;
        }
        Ok(Shape {
            path: builder.finish(),
            points,
        })
    }
}

/// Adds a component outline mapped through `transform`.
fn append(
    builder: &mut PathBuilder,
    path: &Path,
    transform: Transform,
    at: Position,
) -> Result<()> {
    let invalid = || syntax(at, "invalid component path");
    for &segment in path.segments() {
        match segment {
            Segment::MoveTo(p) => {
                builder.move_to(transform.apply(p));
            }
            Segment::LineTo(p) => {
                builder.line_to(transform.apply(p)).ok_or_else(invalid)?;
            }
            Segment::CubicTo(a, b, c) => {
                builder
                    .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                    .ok_or_else(invalid)?;
            }
            Segment::Close => {
                builder.close();
            }
        }
    }
    Ok(())
}

fn simple(r: &mut Cursor<'_>, contours: usize, work: &mut Work<'_>) -> Result<Shape> {
    work.entries(contours, r.position())?;
    let mut ends = Vec::with_capacity(contours);
    let mut count = 0;
    for _ in 0..contours {
        let end = usize::from(r.u16()?);
        if end < count {
            return Err(r.error("unordered contour endpoints"));
        }
        count = end + 1;
        ends.push(count);
    }
    work.work(count, r.position())?;
    work.entries(count, r.position())?;
    work.segments(count.saturating_add(contours * 2), r.position())?;
    let instructions = usize::from(r.u16()?);
    r.skip(instructions)?;
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = r.u8()?;
        if flag & point::RESERVED != 0 {
            return Err(r.error("reserved glyph flag"));
        }
        let repeats = if flag & point::REPEAT != 0 {
            usize::from(r.u8()?) + 1
        } else {
            1
        };
        if repeats > count - flags.len() {
            return Err(r.error("glyph flag repeat exceeds point count"));
        }
        flags.resize(flags.len() + repeats, flag);
    }
    let mut vertices = Vec::with_capacity(count);
    let mut x = 0_i32;
    for &flag in &flags {
        x = x
            .checked_add(delta(r, flag, point::X_SHORT, point::X_SAME_OR_POSITIVE)?)
            .ok_or_else(|| r.error("glyph coordinate overflow"))?;
        vertices.push(Vertex {
            point: Point::new(f64::from(x), 0.0),
            on: flag & point::ON_CURVE != 0,
        });
    }
    let mut y = 0_i32;
    for (vertex, &flag) in vertices.iter_mut().zip(&flags) {
        y = y
            .checked_add(delta(r, flag, point::Y_SHORT, point::Y_SAME_OR_POSITIVE)?)
            .ok_or_else(|| r.error("glyph coordinate overflow"))?;
        vertex.point.y = f64::from(y);
    }
    let at = r.position();
    let mut builder = PathBuilder::new();
    let mut start = 0;
    for end in ends {
        let contour = vertices
            .get(start..end)
            .ok_or_else(|| syntax(at, "contour outside glyph points"))?;
        draw_contour(&mut builder, contour, at)?;
        start = end;
    }
    Ok(Shape {
        path: builder.finish(),
        points: vertices.into_iter().map(|v| v.point).collect(),
    })
}

fn delta(r: &mut Cursor<'_>, flags: u8, short: u8, same: u8) -> Result<i32> {
    if flags & short != 0 {
        let n = i32::from(r.u8()?);
        Ok(if flags & same != 0 { n } else { -n })
    } else if flags & same != 0 {
        Ok(0)
    } else {
        Ok(i32::from(r.i16()?))
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y))
}

/// One contour of quadratic splines: consecutive off-curve points imply
/// an on-curve point midway (OpenType glyf).
fn draw_contour(builder: &mut PathBuilder, vertices: &[Vertex], at: Position) -> Result<()> {
    let (Some(&first), Some(&last)) = (vertices.first(), vertices.last()) else {
        return Err(syntax(at, "empty contour"));
    };
    let (start, rest) = if first.on {
        (first.point, vertices.get(1..).unwrap_or_default())
    } else if last.on {
        (
            last.point,
            vertices.get(..vertices.len() - 1).unwrap_or_default(),
        )
    } else {
        (midpoint(last.point, first.point), vertices)
    };
    builder.move_to(start);
    let mut control = None;
    for vertex in rest {
        if vertex.on {
            if let Some(p) = control.take() {
                quadratic(builder, p, vertex.point, at)?;
            } else {
                builder
                    .line_to(vertex.point)
                    .ok_or_else(|| syntax(at, "line without current point"))?;
            }
        } else {
            if let Some(p) = control {
                quadratic(builder, p, midpoint(p, vertex.point), at)?;
            }
            control = Some(vertex.point);
        }
    }
    if let Some(p) = control {
        quadratic(builder, p, start, at)?;
    }
    builder.close();
    Ok(())
}

/// A quadratic Bézier from the current point, elevated exactly to the
/// cubic with the same curve.
fn quadratic(builder: &mut PathBuilder, control: Point, end: Point, at: Position) -> Result<()> {
    let invalid = || syntax(at, "quadratic without current point");
    let from = builder.current_point().ok_or_else(invalid)?;
    let third = |end: Point| {
        Point::new(
            end.x + (control.x - end.x) * 2.0 / 3.0,
            end.y + (control.y - end.y) * 2.0 / 3.0,
        )
    };
    builder
        .cubic_to(third(from), third(end), end)
        .ok_or_else(invalid)?;
    Ok(())
}

fn component(r: &mut Cursor<'_>, work: &mut Work<'_>) -> Result<Component> {
    let at = r.position();
    let raw = r.u16()?;
    if raw & !component::DEFINED != 0 {
        work.repair(
            repairs::COMPONENT_FLAGS,
            "reserved composite glyph flags ignored",
            at,
        )?;
    }
    let flags = raw & component::DEFINED;
    let offsets = component::SCALED_COMPONENT_OFFSET | component::UNSCALED_COMPONENT_OFFSET;
    if flags & offsets == offsets {
        return Err(r.error("conflicting component offset flags"));
    }
    let glyph = GlyphId(u32::from(r.u16()?));
    let words = flags & component::ARGS_ARE_WORDS != 0;
    let xy = flags & component::ARGS_ARE_XY_VALUES != 0;
    let mut argument = || -> Result<i32> {
        Ok(match (words, xy) {
            (true, true) => i32::from(r.i16()?),
            (true, false) => i32::from(r.u16()?),
            (false, true) => i32::from(r.i8()?),
            (false, false) => i32::from(r.u8()?),
        })
    };
    let arguments = [argument()?, argument()?];
    let scales = [
        component::HAVE_A_SCALE,
        component::HAVE_X_AND_Y_SCALE,
        component::HAVE_TWO_BY_TWO,
    ];
    if scales.into_iter().filter(|flag| flags & flag != 0).count() > 1 {
        return Err(r.error("conflicting component transforms"));
    }
    let mut transform = Transform::IDENTITY;
    if flags & component::HAVE_A_SCALE != 0 {
        transform.a = r.f2dot14()?;
        transform.d = transform.a;
    }
    if flags & component::HAVE_X_AND_Y_SCALE != 0 {
        transform.a = r.f2dot14()?;
        transform.d = r.f2dot14()?;
    }
    if flags & component::HAVE_TWO_BY_TWO != 0 {
        transform.a = r.f2dot14()?;
        transform.b = r.f2dot14()?;
        transform.c = r.f2dot14()?;
        transform.d = r.f2dot14()?;
    }
    Ok(Component {
        flags,
        glyph,
        arguments,
        transform,
    })
}

#[cfg(test)]
mod tests {
    use super::{Glyf, simple};
    use crate::GlyphId;
    use crate::budget::{FontBudget, Work};
    use crate::read::cursor;
    use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};
    use pagura_raster::{Point, Segment};

    fn work<'a>(diagnostics: &'a mut Diagnostics, limits: &Limits) -> Work<'a> {
        Work::new(FontBudget::new(limits), diagnostics)
    }

    #[test]
    fn implied_points_and_closing_quadratics() {
        // One contour: on (0,0), off (100,0), on (100,100), off (0,100).
        // Flags use short and same-coordinate encodings; deltas follow.
        let data = [0, 3, 0, 0, 0x31, 0x32, 0x35, 0x22, 100, 100, 100];
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let shape = simple(
            &mut cursor(&data),
            1,
            &mut work(&mut diagnostics, &Limits::default()),
        )
        .unwrap();
        let p = Point::new;
        assert_eq!(
            shape.points,
            [p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 100.0)]
        );
        let segments = shape.path.segments();
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0], Segment::MoveTo(p(0.0, 0.0)));
        // The trailing off-curve point closes the contour with a quadratic.
        assert!(matches!(segments[2], Segment::CubicTo(_, _, end) if end == p(0.0, 0.0)));
        assert_eq!(segments[3], Segment::Close);
    }

    #[test]
    fn flag_repeats_must_stay_within_the_points() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let limits = Limits::default();
        // Two points: one flag repeated once, all coordinates unchanged.
        let shape = simple(
            &mut cursor(&[0, 1, 0, 0, 0x39, 1]),
            1,
            &mut work(&mut diagnostics, &limits),
        )
        .unwrap();
        assert_eq!(shape.points.len(), 2);
        let error = simple(
            &mut cursor(&[0, 1, 0, 0, 0x39, 2]),
            1,
            &mut work(&mut diagnostics, &limits),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
    }

    #[test]
    fn composite_cycles_are_malformed_and_deep_composites_are_budgeted() {
        // Glyph 0 is a composite of glyph `child`; glyph 1 is empty.
        let composite = |child: u8| [0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, child, 0, 0];
        let glyf = Glyf::new(0..16, vec![Some(0), Some(16), Some(16)]);
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let limits = Limits::default();
        let error = glyf
            .outline(
                &composite(0),
                GlyphId(0),
                &mut work(&mut diagnostics, &limits),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        glyf.outline(
            &composite(1),
            GlyphId(0),
            &mut work(&mut diagnostics, &limits),
        )
        .unwrap();
        let shallow = Limits {
            depth: 0,
            ..Limits::default()
        };
        let error = glyf
            .outline(
                &composite(1),
                GlyphId(0),
                &mut work(&mut diagnostics, &shallow),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}
