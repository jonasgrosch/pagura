//! Type 1 charstrings: unhinted outlines, subroutines and StandardEncoding seac.
use super::{Type1, unsupported};
use crate::GlyphId;
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

struct Work<'a> {
    limits: &'a Limits,
    operations: usize,
    segments: usize,
}
impl Work<'_> {
    fn segment(&mut self, at: usize) -> Result<()> {
        self.segments = self.segments.saturating_add(1);
        cap(
            self.segments,
            self.limits
                .path_segments
                .min(self.limits.decoded_bytes / size_of::<Segment>()),
            at,
        )
    }
}
struct State {
    stack: Vec<f64>,
    results: Vec<f64>,
    path: PathBuilder,
    point: Point,
    width: Option<f64>,
    open: bool,
    seac: Option<[f64; 5]>,
    flex: Option<Vec<Point>>,
}
#[derive(PartialEq)]
enum Flow {
    Return,
    End,
}

pub(super) fn glyph(font: &Type1, glyph: GlyphId, limits: &Limits) -> Result<(Path, f64)> {
    load(
        font,
        glyph,
        0,
        &mut Work {
            limits,
            operations: 0,
            segments: 0,
        },
    )
}
fn load(font: &Type1, glyph: GlyphId, depth: usize, work: &mut Work<'_>) -> Result<(Path, f64)> {
    cap(depth, work.limits.depth.min(64), 0)?;
    let index = usize::try_from(glyph.0).map_err(|_| bad(0, "glyph index overflow"))?;
    let bytes = font
        .glyphs
        .get(index)
        .ok_or_else(|| bad(0, "glyph index outside Type 1 font"))?;
    let mut s = State {
        stack: Vec::new(),
        results: Vec::new(),
        path: PathBuilder::new(),
        point: Point::default(),
        width: None,
        open: false,
        seac: None,
        flex: None,
    };
    run(font, Read::new(bytes, 0), 0, &mut s, work)?;
    if s.flex.is_some() {
        return Err(bad(0, "unterminated Type 1 flex"));
    }
    s.close(work, bytes.len())?;
    if let Some([asb, dx, dy, base, accent]) = s.seac {
        for (code, transform) in [
            (base, Transform::IDENTITY),
            (accent, Transform::translate(dx - asb, dy)),
        ] {
            let code =
                u8::try_from(integer(code, 0)?).map_err(|_| bad(0, "invalid Type 1 seac code"))?;
            let name = crate::cff::strings::standard(code)
                .ok_or_else(|| bad(0, "undefined Type 1 seac code"))?;
            let glyph = font
                .name(name.as_bytes())
                .ok_or_else(|| bad(0, "missing Type 1 seac component"))?;
            let (path, _) = load(font, glyph, depth + 1, work)?;
            for &segment in path.segments() {
                work.segment(0)?;
                match segment {
                    Segment::MoveTo(p) => {
                        s.path.move_to(transform.apply(p));
                    }
                    Segment::LineTo(p) => {
                        s.path
                            .line_to(transform.apply(p))
                            .ok_or_else(|| bad(0, "invalid seac path"))?;
                    }
                    Segment::CubicTo(a, b, c) => {
                        s.path
                            .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                            .ok_or_else(|| bad(0, "invalid seac path"))?;
                    }
                    Segment::Close => {
                        s.path.close();
                    }
                }
            }
        }
    }
    Ok((
        s.path.finish(),
        s.width
            .ok_or_else(|| bad(0, "Type 1 charstring has no width"))?,
    ))
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be an integral u32"
)]
fn integer(n: f64, at: usize) -> Result<usize> {
    if !n.is_finite() || n < 0.0 || n > f64::from(u32::MAX) || n.fract() != 0.0 {
        return Err(bad(at, "invalid charstring integer"));
    }
    usize::try_from(n as u32).map_err(|_| bad(at, "charstring integer overflow"))
}
impl State {
    fn pop(&mut self, at: usize) -> Result<f64> {
        self.stack
            .pop()
            .ok_or_else(|| bad(at, "Type 1 stack underflow"))
    }
    fn count(&self, n: usize, at: usize) -> Result<()> {
        if self.stack.len() == n {
            Ok(())
        } else {
            Err(bad(at, "invalid Type 1 operand count"))
        }
    }
    fn close(&mut self, w: &mut Work<'_>, at: usize) -> Result<()> {
        if self.open {
            w.segment(at)?;
            self.path.close();
            self.open = false;
        }
        Ok(())
    }
    fn relative(&mut self, dx: f64, dy: f64, at: usize) -> Result<Point> {
        self.point.x += dx;
        self.point.y += dy;
        if !self.point.is_finite() {
            return Err(bad(at, "nonfinite Type 1 coordinate"));
        }
        Ok(self.point)
    }
    fn moveto(&mut self, dx: f64, dy: f64, w: &mut Work<'_>, at: usize) -> Result<()> {
        let p = self.relative(dx, dy, at)?;
        if self.flex.is_none() {
            self.close(w, at)?;
            w.segment(at)?;
            self.path.move_to(p);
            self.open = true;
        }
        Ok(())
    }
    fn line(&mut self, dx: f64, dy: f64, w: &mut Work<'_>, at: usize) -> Result<()> {
        if !self.open || self.flex.is_some() {
            return Err(bad(at, "Type 1 line without an open contour"));
        }
        let p = self.relative(dx, dy, at)?;
        w.segment(at)?;
        self.path
            .line_to(p)
            .ok_or_else(|| bad(at, "Type 1 line before moveto"))?;
        Ok(())
    }
    fn curve(&mut self, d: &[f64], w: &mut Work<'_>, at: usize) -> Result<()> {
        if !self.open || self.flex.is_some() {
            return Err(bad(at, "Type 1 curve without an open contour"));
        }
        let a = self.relative(d[0], d[1], at)?;
        let b = self.relative(d[2], d[3], at)?;
        let c = self.relative(d[4], d[5], at)?;
        w.segment(at)?;
        self.path
            .cubic_to(a, b, c)
            .ok_or_else(|| bad(at, "Type 1 curve before moveto"))?;
        Ok(())
    }
}
fn run(
    font: &Type1,
    mut r: Read<'_>,
    depth: usize,
    s: &mut State,
    w: &mut Work<'_>,
) -> Result<Flow> {
    cap(depth, w.limits.depth.min(64), r.at())?;
    while r.pos < r.data.len() {
        let at = r.at();
        w.operations = w.operations.saturating_add(1);
        cap(w.operations, w.limits.operations, at)?;
        let byte = r.u8()?;
        if byte >= 32 {
            cap(s.stack.len() + 1, 24.min(w.limits.value_nodes), at)?;
            let n = match byte {
                32..=246 => f64::from(i16::from(byte) - 139),
                247..=250 => f64::from((u16::from(byte) - 247) * 256 + u16::from(r.u8()?) + 108),
                251..=254 => -f64::from((u16::from(byte) - 251) * 256 + u16::from(r.u8()?) + 108),
                _ => f64::from(r.i32()?),
            };
            s.stack.push(n);
            continue;
        }
        match byte {
            1 | 3 => {
                s.count(2, at)?;
                s.stack.clear();
            }
            4 | 21 | 22 => {
                s.count(if byte == 21 { 2 } else { 1 }, at)?;
                let (dx, dy) = match byte {
                    4 => (0.0, s.stack[0]),
                    22 => (s.stack[0], 0.0),
                    _ => (s.stack[0], s.stack[1]),
                };
                s.stack.clear();
                s.moveto(dx, dy, w, at)?;
            }
            5..=8 | 30 | 31 => draw(byte, s, w, at)?,
            9 => {
                s.count(0, at)?;
                s.close(w, at)?;
            }
            10 => {
                let index = integer(s.pop(at)?, at)?;
                let bytes = font
                    .subrs
                    .get(index)
                    .ok_or_else(|| bad(at, "Type 1 subroutine index out of range"))?;
                if run(font, Read::new(bytes, 0), depth + 1, s, w)? == Flow::End {
                    return Ok(Flow::End);
                }
            }
            11 => {
                if depth == 0 {
                    return Err(bad(at, "Type 1 return outside subroutine"));
                }
                return Ok(Flow::Return);
            }
            12 => {
                if escape(r.u8()?, s, w, at)? {
                    return Ok(Flow::End);
                }
            }
            13 => {
                s.count(2, at)?;
                if s.width.is_some() {
                    return Err(bad(at, "duplicate Type 1 width"));
                }
                s.point = Point::new(s.stack[0], 0.0);
                s.width = Some(s.stack[1]);
                s.stack.clear();
            }
            14 => {
                s.count(0, at)?;
                return Ok(Flow::End);
            }
            _ => return Err(bad(at, "invalid Type 1 charstring operator")),
        }
        cap(s.stack.len(), 24.min(w.limits.value_nodes), at)?;
        // Finite operands are an invariant: arithmetic writes only the top
        // operand, and blend validates every value it writes in place.
        if s.stack.last().is_some_and(|n| !n.is_finite()) {
            return Err(bad(at, "nonfinite Type 1 operand"));
        }
    }
    Err(bad(r.at(), "unterminated Type 1 charstring"))
}
fn draw(op: u8, s: &mut State, w: &mut Work<'_>, at: usize) -> Result<()> {
    let args = std::mem::take(&mut s.stack);
    match (op, args.as_slice()) {
        (5, [x, y]) => s.line(*x, *y, w, at)?,
        (6, [x]) => s.line(*x, 0.0, w, at)?,
        (7, [y]) => s.line(0.0, *y, w, at)?,
        (8, [_, _, _, _, _, _]) => s.curve(&args, w, at)?,
        (30, [a, b, c, d]) => s.curve(&[0.0, *a, *b, *c, *d, 0.0], w, at)?,
        (31, [a, b, c, d]) => s.curve(&[*a, 0.0, *b, *c, 0.0, *d], w, at)?,
        _ => return Err(bad(at, "invalid Type 1 drawing operands")),
    }
    Ok(())
}
fn escape(op: u8, s: &mut State, w: &mut Work<'_>, at: usize) -> Result<bool> {
    match op {
        0 => s.count(0, at)?, // dotsection affects hints only.
        1 | 2 => {
            s.count(6, at)?;
            s.stack.clear();
        }
        6 => {
            s.count(5, at)?;
            s.seac = Some([s.stack[0], s.stack[1], s.stack[2], s.stack[3], s.stack[4]]);
            s.stack.clear();
            return Ok(true);
        }
        7 => {
            s.count(4, at)?;
            if s.width.is_some() {
                return Err(bad(at, "duplicate Type 1 width"));
            }
            if s.stack[3] != 0.0 {
                return Err(unsupported(
                    at,
                    "nonhorizontal Type 1 advance is not implemented",
                ));
            }
            s.point = Point::new(s.stack[0], s.stack[1]);
            s.width = Some(s.stack[2]);
            s.stack.clear();
        }
        12 => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.push(a / b);
        }
        16 => othersubr(s, w, at)?,
        17 => {
            let value = s
                .results
                .pop()
                .ok_or_else(|| bad(at, "Type 1 othersubr result underflow"))?;
            s.stack.push(value);
        }
        33 => {
            s.count(2, at)?;
            s.point = Point::new(s.stack[0], s.stack[1]);
            s.stack.clear();
        }
        _ => return Err(bad(at, "invalid escaped Type 1 operator")),
    }
    Ok(false)
}
fn othersubr(s: &mut State, w: &mut Work<'_>, at: usize) -> Result<()> {
    let subr = integer(s.pop(at)?, at)?;
    let count = integer(s.pop(at)?, at)?;
    if count > s.stack.len() {
        return Err(bad(at, "Type 1 othersubr stack underflow"));
    }
    let args = s.stack.split_off(s.stack.len() - count);
    match (subr, args.as_slice()) {
        (1, []) => {
            if s.flex.is_some() {
                return Err(bad(at, "nested Type 1 flex"));
            }
            s.flex = Some(Vec::new());
        }
        (2, []) => {
            let points = s
                .flex
                .as_mut()
                .ok_or_else(|| bad(at, "Type 1 flex point without flex start"))?;
            cap(points.len() + 1, 7, at)?;
            points.push(s.point);
        }
        (0, [_, x, y]) => {
            let points = s
                .flex
                .take()
                .ok_or_else(|| bad(at, "Type 1 flex end without start"))?;
            let [_, a, b, c, d, e, f] = points.as_slice() else {
                return Err(bad(at, "Type 1 flex needs seven points"));
            };
            w.segment(at)?;
            s.path
                .cubic_to(*a, *b, *c)
                .ok_or_else(|| bad(at, "Type 1 flex before moveto"))?;
            w.segment(at)?;
            s.path
                .cubic_to(*d, *e, *f)
                .ok_or_else(|| bad(at, "Type 1 flex before moveto"))?;
            s.results.extend([*y, *x]);
        }
        (3, [_]) => {
            s.results.extend(args);
        } // Hint replacement returns the subroutine number.
        _ => {
            return Err(unsupported(
                at,
                "Type 1 OtherSubrs procedure is not implemented",
            ));
        }
    }
    cap(s.results.len(), 24.min(w.limits.value_nodes), at)
}
