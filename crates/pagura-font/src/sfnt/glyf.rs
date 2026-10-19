//! Unhinted TrueType contours, implied quadratic points, and components.
use super::Sfnt;
use crate::GlyphId;
use crate::read::{bad, cap, here, slice};
use crate::{recovery::repair, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostics, Limits, Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

#[derive(Clone, Copy)]
struct Vertex {
    point: Point,
    on: bool,
}

struct Shape {
    path: Path,
    points: Vec<Point>,
}
struct Work<'a> {
    limits: &'a Limits,
    diagnostics: &'a mut Diagnostics,
    operations: usize,
    points: usize,
    segments: usize,
}
impl Work<'_> {
    fn charge(
        &mut self,
        operations: usize,
        points: usize,
        segments: usize,
        at: usize,
    ) -> Result<()> {
        self.operations = self.operations.saturating_add(operations);
        self.points = self.points.saturating_add(points);
        self.segments = self.segments.saturating_add(segments);
        cap(self.operations, self.limits.operations, at)?;
        cap(
            self.points,
            self.limits
                .value_nodes
                .min(self.limits.decoded_bytes / size_of::<Vertex>()),
            at,
        )?;
        cap(self.segments, self.limits.path_segments, at)
    }
}

pub(super) fn outline(
    font: &Sfnt,
    data: &[u8],
    glyph: GlyphId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Path> {
    let mut work = Work {
        limits,
        diagnostics,
        operations: 0,
        points: 0,
        segments: 0,
    };
    Ok(load(font, data, glyph, 0, &mut work)?.path)
}

fn load(
    font: &Sfnt,
    data: &[u8],
    glyph: GlyphId,
    depth: usize,
    work: &mut Work<'_>,
) -> Result<Shape> {
    cap(depth, work.limits.depth.min(64), font.glyf.start)?;
    work.charge(1, 0, 0, font.glyf.start)?;
    let index =
        usize::try_from(glyph.0).map_err(|_| crate::read::limit(0, "glyph index overflow"))?;
    let start = font
        .locations
        .get(index)
        .copied()
        .flatten()
        .ok_or_else(|| bad(font.glyf.start, "missing glyph location"))?;
    let end = font
        .locations
        .get(index + 1)
        .copied()
        .flatten()
        .ok_or_else(|| bad(font.glyf.start, "missing glyph location"))?;
    if start == end {
        return Ok(Shape {
            path: Path::default(),
            points: Vec::new(),
        });
    }
    if start > font.glyf.len() || end > font.glyf.len() {
        return Err(bad(font.glyf.end, "truncated glyph"));
    }
    let base = font.glyf.start + start;
    let mut r = slice(data, base..font.glyf.start + end, "truncated glyph")?;
    let contours = r.i16()?;
    r.skip(8)?;
    match contours {
        0.. => simple(
            &mut r,
            usize::try_from(contours).map_err(|_| bad(base, "invalid contour count"))?,
            work,
        ),
        -1 => composite(font, data, &mut r, depth, work),
        _ => Err(bad(base, "invalid negative contour count")),
    }
}

fn simple(r: &mut Cursor<'_>, contours: usize, work: &mut Work<'_>) -> Result<Shape> {
    cap(contours, work.limits.value_nodes, here(r))?;
    let mut ends = Vec::with_capacity(contours);
    let mut count = 0;
    for _ in 0..contours {
        let end = usize::from(r.u16()?);
        if end < count {
            return Err(bad(here(r), "unordered contour endpoints"));
        }
        count = end + 1;
        ends.push(count);
    }
    work.charge(count, count, count.saturating_add(contours * 2), here(r))?;
    let instructions = usize::from(r.u16()?);
    r.skip(instructions)?;
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = r.u8()?;
        if flag & 0x80 != 0 {
            return Err(bad(here(r), "reserved glyph flag"));
        }
        let repeats = if flag & 8 != 0 {
            usize::from(r.u8()?) + 1
        } else {
            1
        };
        if repeats > count - flags.len() {
            return Err(bad(here(r), "glyph flag repeat exceeds point count"));
        }
        flags.resize(flags.len() + repeats, flag);
    }
    let mut vertices = Vec::with_capacity(count);
    let mut x = 0_i32;
    for &flag in &flags {
        x = x
            .checked_add(delta(r, flag, 2, 16)?)
            .ok_or_else(|| crate::read::limit(here(r), "glyph coordinate overflow"))?;
        vertices.push(Vertex {
            point: Point::new(f64::from(x), 0.0),
            on: flag & 1 != 0,
        });
    }
    let mut y = 0_i32;
    for (vertex, &flag) in vertices.iter_mut().zip(&flags) {
        y = y
            .checked_add(delta(r, flag, 4, 32)?)
            .ok_or_else(|| crate::read::limit(here(r), "glyph coordinate overflow"))?;
        vertex.point.y = f64::from(y);
    }
    let mut builder = PathBuilder::new();
    let mut start = 0;
    for end in ends {
        contour(&mut builder, &vertices[start..end])?;
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
fn quadratic(builder: &mut PathBuilder, control: Point, end: Point) -> Result<()> {
    let from = builder
        .current_point()
        .ok_or_else(|| bad(0, "quadratic without current point"))?;
    let a = Point::new(
        from.x + (control.x - from.x) * 2.0 / 3.0,
        from.y + (control.y - from.y) * 2.0 / 3.0,
    );
    let b = Point::new(
        end.x + (control.x - end.x) * 2.0 / 3.0,
        end.y + (control.y - end.y) * 2.0 / 3.0,
    );
    builder
        .cubic_to(a, b, end)
        .ok_or_else(|| bad(0, "quadratic without current point"))?;
    Ok(())
}
fn contour(builder: &mut PathBuilder, vertices: &[Vertex]) -> Result<()> {
    let first = *vertices.first().ok_or_else(|| bad(0, "empty contour"))?;
    let last = *vertices.last().ok_or_else(|| bad(0, "empty contour"))?;
    let (start, skip, take) = if first.on {
        (first.point, 1, vertices.len() - 1)
    } else if last.on {
        (last.point, 0, vertices.len() - 1)
    } else {
        (midpoint(last.point, first.point), 0, vertices.len())
    };
    builder.move_to(start);
    let mut control = None;
    for vertex in vertices.iter().skip(skip).take(take) {
        if vertex.on {
            if let Some(p) = control.take() {
                quadratic(builder, p, vertex.point)?;
            } else {
                builder
                    .line_to(vertex.point)
                    .ok_or_else(|| bad(0, "line without current point"))?;
            }
        } else {
            if let Some(p) = control {
                quadratic(builder, p, midpoint(p, vertex.point))?;
            }
            control = Some(vertex.point);
        }
    }
    if let Some(p) = control {
        quadratic(builder, p, start)?;
    }
    builder.close();
    Ok(())
}

fn composite(
    font: &Sfnt,
    data: &[u8],
    r: &mut Cursor<'_>,
    depth: usize,
    work: &mut Work<'_>,
) -> Result<Shape> {
    let mut points = Vec::new();
    let mut builder = PathBuilder::new();
    let mut have_instructions = false;
    loop {
        work.charge(1, 0, 0, here(r))?;
        let Component {
            flags,
            glyph,
            a,
            b,
            mut transform,
        } = component(r, work.diagnostics)?;
        let xy = flags & 2 != 0;
        let shape = load(font, data, glyph, depth + 1, work)?;
        let offset = if xy {
            let p = Point::new(f64::from(a), f64::from(b));
            // OpenType leaves legacy offsets unspecified; use unscaled offsets
            // unless SCALED_COMPONENT_OFFSET explicitly requests scaling.
            if flags & 0x800 != 0 {
                transform.apply(p)
            } else {
                p
            }
        } else {
            let parent: Point = *points
                .get(usize::try_from(a).map_err(|_| bad(here(r), "invalid component point"))?)
                .ok_or_else(|| bad(here(r), "missing parent attachment point"))?;
            let child = transform.apply(
                *shape
                    .points
                    .get(usize::try_from(b).map_err(|_| bad(here(r), "invalid component point"))?)
                    .ok_or_else(|| bad(here(r), "missing child attachment point"))?,
            );
            Point::new(parent.x - child.x, parent.y - child.y)
        };
        transform.e = if flags & 4 != 0 {
            offset.x.round()
        } else {
            offset.x
        };
        transform.f = if flags & 4 != 0 {
            offset.y.round()
        } else {
            offset.y
        };
        work.charge(
            shape.path.segments().len(),
            shape.points.len(),
            shape.path.segments().len(),
            here(r),
        )?;
        for &p in &shape.points {
            let p = transform.apply(p);
            if !p.is_finite() {
                return Err(crate::read::limit(here(r), "nonfinite component point"));
            }
            points.push(p);
        }
        for &segment in shape.path.segments() {
            match segment {
                Segment::MoveTo(p) => {
                    builder.move_to(transform.apply(p));
                }
                Segment::LineTo(p) => {
                    builder
                        .line_to(transform.apply(p))
                        .ok_or_else(|| bad(here(r), "invalid component path"))?;
                }
                Segment::CubicTo(a, b, c) => {
                    builder
                        .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                        .ok_or_else(|| bad(here(r), "invalid component path"))?;
                }
                Segment::Close => {
                    builder.close();
                }
            }
        }
        have_instructions |= flags & 256 != 0;
        if flags & 32 == 0 {
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

struct Component {
    flags: u16,
    glyph: GlyphId,
    a: i32,
    b: i32,
    transform: Transform,
}
fn component(r: &mut Cursor<'_>, diagnostics: &mut Diagnostics) -> Result<Component> {
    let raw = r.u16()?;
    if raw & !0x1fef != 0 {
        repair(
            diagnostics,
            repairs::COMPONENT_FLAGS,
            "reserved composite glyph flags ignored",
            Position::Buffer(here(r) - 2),
        )?;
    }
    let flags = raw & 0x1fef;
    if flags & 0x1800 == 0x1800 {
        return Err(bad(here(r), "conflicting component offset flags"));
    }
    let glyph = GlyphId(u32::from(r.u16()?));
    let words = flags & 1 != 0;
    let xy = flags & 2 != 0;
    let mut argument = || -> Result<i32> {
        if words {
            if xy {
                Ok(i32::from(r.i16()?))
            } else {
                Ok(i32::from(r.u16()?))
            }
        } else {
            let byte = r.u8()?;
            Ok(if xy {
                i32::from(i8::from_ne_bytes([byte]))
            } else {
                i32::from(byte)
            })
        }
    };
    let a = argument()?;
    let b = argument()?;
    let mut transform = Transform::IDENTITY;
    if [8, 64, 128]
        .into_iter()
        .filter(|flag| flags & flag != 0)
        .count()
        > 1
    {
        return Err(bad(here(r), "conflicting component transforms"));
    }
    let mut fixed = || -> Result<f64> { Ok(f64::from(r.i16()?) / 16384.0) };
    if flags & 8 != 0 {
        transform.a = fixed()?;
        transform.d = transform.a;
    }
    if flags & 64 != 0 {
        transform.a = fixed()?;
        transform.d = fixed()?;
    }
    if flags & 128 != 0 {
        transform.a = fixed()?;
        transform.b = fixed()?;
        transform.c = fixed()?;
        transform.d = fixed()?;
    }
    Ok(Component {
        flags,
        glyph,
        a,
        b,
        transform,
    })
}
