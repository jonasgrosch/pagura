//! Type 1 charstrings: unhinted outlines, subroutines, flex and
//! StandardEncoding seac (Type 1 chapters 6 and 8).
use super::Type1;
use crate::budget::Work;
use crate::cff::strings;
use crate::read::{limit, syntax, unsupported};
use crate::{GlyphId, features, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

mod operator;
use operator::{Escape, Operator};

/// Type 1 Appendix 1: 24 operands and 10 nested subroutine calls.
const MAX_OPERANDS: usize = 24;
const MAX_SUBR_DEPTH: usize = 10;
/// Byte 255 introduces a 32-bit integer (Type 1 6.2).
const LONG_INT: u8 = 255;
/// The command bytes that may precede `hsbw` or `sbw` (Type 1 6.4).
const CALLSUBR: u8 = 10;
const ESCAPE: u8 = 12;
const HSBW: u8 = 13;
/// `pop` (12 17) reads OtherSubr results; any other command discards them.
const POP: u8 = 17;
/// The standard OtherSubrs (Type 1 chapter 8) and flex's point count.
const FLEX_END: usize = 0;
const FLEX_START: usize = 1;
const FLEX_POINT: usize = 2;
const HINT_REPLACEMENT: usize = 3;
const FLEX_POINTS: usize = 7;

struct State {
    stack: Vec<f64>,
    results: Vec<f64>,
    path: PathBuilder,
    point: Point,
    width: Option<f64>,
    bearing: Point,
    open: bool,
    seac: Option<[f64; 5]>,
    flex: Option<Vec<Point>>,
    /// A repair ended the glyph before its end.
    stopped: bool,
}
#[derive(PartialEq)]
enum Flow {
    Return,
    End,
}

pub(super) fn outline(font: &Type1, glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
    Ok(load(font, glyph, 0, work)?.0)
}

/// The glyph's width, from the whole interpreted glyph.
pub(super) fn advance(font: &Type1, glyph: GlyphId, work: &mut Work<'_>) -> Result<f64> {
    Ok(load(font, glyph, 0, work)?.1)
}

fn load(
    font: &Type1,
    glyph: GlyphId,
    depth: usize,
    work: &mut Work<'_>,
) -> Result<(Path, f64, Point)> {
    let charstring = font
        .glyphs
        .get(u32_to_usize(glyph.0))
        .ok_or_else(|| syntax(Position::Unknown, "glyph index outside Type 1 font"))?;
    let r = charstring.cursor();
    let (start, end) = (r.position(), Position::Buffer(charstring.end()));
    work.depth(depth, usize::MAX, start)?;
    let mut s = State {
        stack: Vec::new(),
        results: Vec::new(),
        path: PathBuilder::new(),
        point: Point::default(),
        width: None,
        bearing: Point::default(),
        open: false,
        seac: None,
        flex: None,
        stopped: false,
    };
    run(font, r, 0, &mut s, work)?;
    if s.flex.is_some() {
        return Err(syntax(start, "unterminated Type 1 flex"));
    }
    s.close(work, end)?;
    if let Some([asb, dx, dy, base, accent]) = s.seac {
        if depth != 0 {
            return Err(syntax(start, "nested Type 1 seac composite"));
        }
        for (code, transform, is_base) in [
            (base, Transform::IDENTITY, true),
            (accent, Transform::translate(dx, dy), false),
        ] {
            let code = u8::try_from(integer(code, start)?)
                .map_err(|_| syntax(start, "invalid Type 1 seac code"))?;
            let name = strings::standard(code)
                .ok_or_else(|| syntax(start, "undefined Type 1 seac code"))?;
            let glyph = font
                .name(name.as_bytes())
                .ok_or_else(|| syntax(start, "missing Type 1 seac component"))?;
            let (path, width, bearing) = load(font, glyph, depth + 1, work)?;
            // Components already include their side bearings, so the
            // accent origin moves by (adx, ady) only (Type 1 3.1, 6.4).
            if is_base && (Some(width) != s.width || bearing != s.bearing)
                || !is_base && bearing.x != asb
            {
                return Err(syntax(start, "inconsistent Type 1 seac metrics"));
            }
            for &segment in path.segments() {
                work.segments(1, start)?;
                match segment {
                    Segment::MoveTo(p) => {
                        s.path.move_to(transform.apply(p));
                    }
                    Segment::LineTo(p) => {
                        s.path
                            .line_to(transform.apply(p))
                            .ok_or_else(|| syntax(start, "invalid seac path"))?;
                    }
                    Segment::CubicTo(a, b, c) => {
                        s.path
                            .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                            .ok_or_else(|| syntax(start, "invalid seac path"))?;
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
            .ok_or_else(|| syntax(start, "Type 1 charstring has no width"))?,
        s.bearing,
    ))
}
/// A nonnegative integral operand used as an index, count or offset.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be an integral u32"
)]
fn integer(n: f64, at: Position) -> Result<usize> {
    if !n.is_finite() || n < 0.0 || n > f64::from(u32::MAX) || n.fract() != 0.0 {
        return Err(syntax(at, "invalid charstring integer"));
    }
    usize::try_from(n as u32).map_err(|_| limit(at, "charstring integer overflow"))
}
impl State {
    fn pop(&mut self, at: Position) -> Result<f64> {
        self.stack
            .pop()
            .ok_or_else(|| syntax(at, "Type 1 stack underflow"))
    }
    fn count(&self, n: usize, at: Position) -> Result<()> {
        if self.stack.len() == n {
            Ok(())
        } else {
            Err(syntax(at, "invalid Type 1 operand count"))
        }
    }
    fn close(&mut self, w: &mut Work<'_>, at: Position) -> Result<()> {
        if self.open {
            w.segments(1, at)?;
            self.path.close();
            self.open = false;
        }
        Ok(())
    }
    fn relative(&mut self, dx: f64, dy: f64, at: Position) -> Result<Point> {
        self.point.x += dx;
        self.point.y += dy;
        if !self.point.is_finite() {
            return Err(syntax(at, "nonfinite Type 1 coordinate"));
        }
        Ok(self.point)
    }
    fn moveto(&mut self, dx: f64, dy: f64, w: &mut Work<'_>, at: Position) -> Result<()> {
        let p = self.relative(dx, dy, at)?;
        // During flex, moves only collect points (Type 1 8.3).
        if self.flex.is_none() {
            self.close(w, at)?;
            w.segments(1, at)?;
            self.path.move_to(p);
            self.open = true;
        }
        Ok(())
    }
    // Type 1 6.4 keeps the charstring current point across closepath.
    // PathBuilder uses PDF close semantics, so a resumed contour must
    // explicitly begin at the retained Type 1 point.
    fn resume(&mut self, w: &mut Work<'_>, at: Position) -> Result<()> {
        if self.flex.is_some() || self.path.current_point().is_none() {
            return Err(syntax(at, "Type 1 drawing without a current path"));
        }
        if !self.open {
            w.segments(1, at)?;
            self.path.move_to(self.point);
            self.open = true;
        }
        Ok(())
    }
    fn line(&mut self, dx: f64, dy: f64, w: &mut Work<'_>, at: Position) -> Result<()> {
        self.resume(w, at)?;
        let p = self.relative(dx, dy, at)?;
        w.segments(1, at)?;
        self.path
            .line_to(p)
            .ok_or_else(|| syntax(at, "Type 1 line before moveto"))?;
        Ok(())
    }
    fn curve(
        &mut self,
        [dxa, dya, dxb, dyb, dxc, dyc]: [f64; 6],
        w: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        self.resume(w, at)?;
        let a = self.relative(dxa, dya, at)?;
        let b = self.relative(dxb, dyb, at)?;
        let c = self.relative(dxc, dyc, at)?;
        w.segments(1, at)?;
        self.path
            .cubic_to(a, b, c)
            .ok_or_else(|| syntax(at, "Type 1 curve before moveto"))?;
        Ok(())
    }
    fn set_width(&mut self, bearing: Point, width: f64, at: Position) -> Result<()> {
        if self.width.is_some() {
            return Err(syntax(at, "duplicate Type 1 width"));
        }
        self.point = bearing;
        self.bearing = bearing;
        self.width = Some(width);
        self.stack.clear();
        Ok(())
    }
}
fn run(
    font: &Type1,
    mut r: Cursor<'_>,
    depth: usize,
    s: &mut State,
    w: &mut Work<'_>,
) -> Result<Flow> {
    w.depth(depth, MAX_SUBR_DEPTH, r.position())?;
    while !r.at_end() {
        let at = r.position();
        w.work(1, at)?;
        let byte = r.u8()?;
        if byte >= 32 {
            if s.stack.len() == MAX_OPERANDS {
                return Err(syntax(at, "operand stack exceeds the format limit"));
            }
            let n = match byte {
                LONG_INT => f64::from(r.i32()?),
                247..=250 => f64::from((u16::from(byte) - 247) * 256 + u16::from(r.u8()?) + 108),
                251..=254 => -f64::from((u16::from(byte) - 251) * 256 + u16::from(r.u8()?) + 108),
                _ => f64::from(i16::from(byte) - 139),
            };
            s.stack.push(n);
            continue;
        }
        if s.width.is_none() && !matches!(byte, CALLSUBR | ESCAPE | HSBW) {
            return Err(syntax(at, "Type 1 width must precede outline operators"));
        }
        if byte != ESCAPE || r.rest().first() != Some(&POP) {
            s.results.clear();
        }
        let op = Operator::try_from(byte)
            .map_err(|_| syntax(at, "invalid Type 1 charstring operator"))?;
        match op {
            Operator::Hstem | Operator::Vstem => {
                s.count(2, at)?;
                s.stack.clear();
            }
            Operator::Vmoveto | Operator::Rmoveto | Operator::Hmoveto => {
                let (dx, dy) = match (op, s.stack.as_slice()) {
                    (Operator::Vmoveto, &[dy]) => (0.0, dy),
                    (Operator::Hmoveto, &[dx]) => (dx, 0.0),
                    (Operator::Rmoveto, &[dx, dy]) => (dx, dy),
                    _ => return Err(syntax(at, "invalid Type 1 operand count")),
                };
                s.stack.clear();
                s.moveto(dx, dy, w, at)?;
            }
            Operator::Closepath => {
                s.count(0, at)?;
                s.close(w, at)?;
            }
            Operator::Callsubr => {
                let index = integer(s.pop(at)?, at)?;
                let subr = font
                    .subrs
                    .get(index)
                    .ok_or_else(|| syntax(at, "Type 1 subroutine index out of range"))?;
                if run(font, subr.cursor(), depth + 1, s, w)? == Flow::End {
                    if !s.stopped && !r.at_end() {
                        return Err(r.error("data after Type 1 character end"));
                    }
                    return Ok(Flow::End);
                }
            }
            Operator::Return => {
                if depth == 0 {
                    return Err(syntax(at, "Type 1 return outside subroutine"));
                }
                return Ok(Flow::Return);
            }
            Operator::Escape => {
                if escape(font, r.u8()?, s, w, at)? {
                    if !s.stopped && !r.at_end() {
                        return Err(r.error("data after Type 1 seac"));
                    }
                    return Ok(Flow::End);
                }
            }
            Operator::Hsbw => {
                let &[sbx, wx] = s.stack.as_slice() else {
                    return Err(syntax(at, "invalid Type 1 operand count"));
                };
                s.set_width(Point::new(sbx, 0.0), wx, at)?;
            }
            Operator::Endchar => {
                s.count(0, at)?;
                if !s.stopped && !r.at_end() {
                    return Err(r.error("data after Type 1 endchar"));
                }
                return Ok(Flow::End);
            }
            _ => draw(op, s, w, at)?,
        }
        if s.stack.len() > MAX_OPERANDS {
            return Err(syntax(at, "operand stack exceeds the format limit"));
        }
        // Finite operands are an invariant: arithmetic writes only the top
        // operand, and blend validates every value it writes in place.
        if s.stack.last().is_some_and(|n| !n.is_finite()) {
            return Err(syntax(at, "nonfinite Type 1 operand"));
        }
    }
    Err(r.error("unterminated Type 1 charstring"))
}
fn draw(op: Operator, s: &mut State, w: &mut Work<'_>, at: Position) -> Result<()> {
    let args = std::mem::take(&mut s.stack);
    let curve = match (op, args.as_slice()) {
        (Operator::Rlineto, &[dx, dy]) => return s.line(dx, dy, w, at),
        (Operator::Hlineto, &[dx]) => return s.line(dx, 0.0, w, at),
        (Operator::Vlineto, &[dy]) => return s.line(0.0, dy, w, at),
        (Operator::Rrcurveto, &[a, b, c, d, e, f]) => [a, b, c, d, e, f],
        (Operator::Vhcurveto, &[a, b, c, d]) => [0.0, a, b, c, d, 0.0],
        (Operator::Hvcurveto, &[a, b, c, d]) => [a, 0.0, b, c, 0.0, d],
        _ => return Err(syntax(at, "invalid Type 1 drawing operands")),
    };
    s.curve(curve, w, at)
}
/// An escaped command; `true` if it ends the glyph (`seac`, or an unknown
/// command, which ends it as a reported repair).
fn escape(font: &Type1, code: u8, s: &mut State, w: &mut Work<'_>, at: Position) -> Result<bool> {
    let op = Escape::try_from(code);
    if s.width.is_none() && !matches!(op, Ok(Escape::Sbw | Escape::Div)) {
        return Err(syntax(at, "Type 1 width must be the first command"));
    }
    let Ok(op) = op else {
        w.repair(
            repairs::CHARSTRING_OPERATOR,
            "unknown escaped Type 1 operator ends glyph",
            font.input(at),
        )?;
        s.stack.clear();
        s.stopped = true;
        return Ok(true);
    };
    match op {
        Escape::Dotsection => s.count(0, at)?, // Dotsection affects hints only.
        Escape::Vstem3 | Escape::Hstem3 => {
            s.count(6, at)?;
            s.stack.clear();
        }
        Escape::Seac => {
            let &[asb, dx, dy, base, accent] = s.stack.as_slice() else {
                return Err(syntax(at, "invalid Type 1 operand count"));
            };
            s.seac = Some([asb, dx, dy, base, accent]);
            s.stack.clear();
            return Ok(true);
        }
        Escape::Sbw => {
            let &[sbx, sby, wx, wy] = s.stack.as_slice() else {
                return Err(syntax(at, "invalid Type 1 operand count"));
            };
            if s.width.is_some() {
                return Err(syntax(at, "duplicate Type 1 width"));
            }
            if wy != 0.0 {
                return Err(unsupported(
                    features::TYPE1_POSTSCRIPT,
                    at,
                    "nonhorizontal Type 1 advance is not implemented",
                ));
            }
            s.set_width(Point::new(sbx, sby), wx, at)?;
        }
        Escape::Div => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.push(a / b);
        }
        Escape::Callothersubr => othersubr(s, w, at)?,
        Escape::Pop => {
            let value = s
                .results
                .pop()
                .ok_or_else(|| syntax(at, "Type 1 othersubr result underflow"))?;
            s.stack.push(value);
        }
        Escape::Setcurrentpoint => {
            let &[x, y] = s.stack.as_slice() else {
                return Err(syntax(at, "invalid Type 1 operand count"));
            };
            s.point = Point::new(x, y);
            s.stack.clear();
        }
    }
    Ok(false)
}
/// `callothersubr`: the standard flex and hint-replacement OtherSubrs,
/// and chapter 8's argument fallback for the others.
fn othersubr(s: &mut State, w: &mut Work<'_>, at: Position) -> Result<()> {
    let subr = integer(s.pop(at)?, at)?;
    let count = integer(s.pop(at)?, at)?;
    let split = s
        .stack
        .len()
        .checked_sub(count)
        .ok_or_else(|| syntax(at, "Type 1 othersubr stack underflow"))?;
    let args = s.stack.split_off(split);
    match (subr, args.as_slice()) {
        (FLEX_START, []) => {
            if s.flex.is_some() {
                return Err(syntax(at, "nested Type 1 flex"));
            }
            s.flex = Some(Vec::new());
        }
        (FLEX_POINT, []) => {
            let points = s
                .flex
                .as_mut()
                .ok_or_else(|| syntax(at, "Type 1 flex point without flex start"))?;
            if points.len() == FLEX_POINTS {
                return Err(syntax(at, "Type 1 flex has more than seven points"));
            }
            points.push(s.point);
        }
        (FLEX_END, &[_, x, y]) => {
            let points = s
                .flex
                .take()
                .ok_or_else(|| syntax(at, "Type 1 flex end without start"))?;
            let &[_reference, a, b, c, d, e, f] = points.as_slice() else {
                return Err(syntax(at, "Type 1 flex needs seven points"));
            };
            w.segments(1, at)?;
            s.path
                .cubic_to(a, b, c)
                .ok_or_else(|| syntax(at, "Type 1 flex before moveto"))?;
            w.segments(1, at)?;
            s.path
                .cubic_to(d, e, f)
                .ok_or_else(|| syntax(at, "Type 1 flex before moveto"))?;
            s.results.extend([y, x]);
        }
        // Hint replacement returns the subroutine number.
        (HINT_REPLACEMENT, [_]) => s.results.extend(args),
        (FLEX_END..=HINT_REPLACEMENT, _) => {
            return Err(syntax(at, "invalid standard Type 1 OtherSubr arguments"));
        }
        // Type 1 chapter 8 fallback: the arguments return in order, arg1
        // popped first.
        _ => s.results.extend(args.into_iter().rev()),
    }
    if s.results.len() > MAX_OPERANDS {
        return Err(syntax(at, "operand stack exceeds the format limit"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::locator::Locator;
    use super::super::{Charstring, Type1};
    use super::{advance, outline};
    use crate::GlyphId;
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};
    use pagura_raster::{Point, Segment, Transform};
    use std::collections::BTreeMap;

    fn font(charstring: &[u8]) -> Type1 {
        Type1 {
            glyphs: vec![Charstring {
                bytes: charstring.to_vec(),
                at: 0,
            }],
            glyph_count: 1,
            subrs: Vec::new(),
            names: BTreeMap::new(),
            glyph_names: Vec::new(),
            encoding: Box::new(std::array::from_fn(|_| None)),
            matrix: Transform::IDENTITY,
            locator: Locator::default(),
        }
    }

    fn run(charstring: &[u8]) -> pagura_core::Result<pagura_raster::Path> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        outline(
            &font(charstring),
            GlyphId(0),
            &mut Work::new(FontBudget::new(&Limits::default()), &mut diagnostics),
        )
    }

    // 0 500 hsbw: side bearing 0, width 500 (Type 1 6.2 number encodings).
    const HSBW: [u8; 4] = [139, 248, 136, 13];

    #[test]
    fn widths_paths_and_closepath() {
        // 100 100 rmoveto 200 0 rlineto closepath endchar
        let mut program = HSBW.to_vec();
        program.extend([239, 239, 21, 247, 92, 139, 5, 9, 14]);
        let path = run(&program).unwrap();
        let p = Point::new;
        assert_eq!(
            path.segments(),
            [
                Segment::MoveTo(p(100.0, 100.0)),
                Segment::LineTo(p(300.0, 100.0)),
                Segment::Close
            ]
        );
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        assert_eq!(
            advance(&font(&program), GlyphId(0), &mut work).unwrap(),
            500.0
        );
        // Without hsbw, drawing is malformed.
        assert_eq!(
            run(&[239, 239, 21, 14]).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn unknown_othersubrs_return_their_arguments_first_to_last() {
        // 7 8 2 99 callothersubr pop pop rmoveto closepath endchar
        let mut program = HSBW.to_vec();
        program.extend([146, 147, 141, 238, 12, 16, 12, 17, 12, 17, 21, 9, 14]);
        let path = run(&program).unwrap();
        assert_eq!(path.segments()[0], Segment::MoveTo(Point::new(7.0, 8.0)));
    }

    #[test]
    fn the_operand_stack_ceiling_is_24() {
        let mut program = HSBW.to_vec();
        program.extend([139; 25]);
        assert_eq!(run(&program).unwrap_err().kind, ErrorKind::Syntax);
    }
}
