//! Bounded, unhinted Type 2 charstrings (Adobe Technical Note 5177).
use super::{Cff, Private, Version, dict, strings};
use crate::budget::Work;
use crate::read::{here, limit, syntax};
use crate::{GlyphId, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

mod operator;
use operator::{Escape, Operator};

/// TN 5177 Appendix B: operand stack, subroutine nesting, stem hints,
/// transient array and charstring length.
const MAX_OPERANDS: usize = 48;
const MAX_SUBR_DEPTH: usize = 10;
const MAX_STEM_HINTS: usize = 96;
const TRANSIENT_SLOTS: usize = 32;
const MAX_CHARSTRING_BYTES: usize = 65_535;
/// The CFF2 operand stack (OpenType CFF2). CFF2 nesting has no verified
/// format ceiling; `Limits::depth` bounds it.
const MAX_CFF2_OPERANDS: usize = 513;
/// The number encoding introduced by byte 28: a 16-bit integer (TN 5177 3.2).
const SHORT_INT: u8 = 28;
/// Byte 255 introduces a 16.16 fixed-point number (TN 5177 3.2).
const FIXED: u8 = 255;
/// `random` is a 32-bit linear congruential generator (implementation
/// policy: TN 5177 4.4 only fixes the range (0, 1]).
const RANDOM_MULTIPLIER: u32 = 1_664_525;
const RANDOM_INCREMENT: u32 = 1_013_904_223;
const RANDOM_RANGE: f64 = 4_294_967_296.0;

struct State {
    stack: Vec<f64>,
    transient: [f64; TRANSIENT_SLOTS],
    path: PathBuilder,
    point: Point,
    open: bool,
    width: Option<f64>,
    hints: usize,
    seac: Option<[f64; 4]>,
    vsindex: usize,
    hint_phase: HintPhase,
    random: u32,
    /// A repair ended the glyph before its end.
    stopped: bool,
}
// TN 5177 section 3.1 orders declarations before masks and path construction.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HintPhase {
    Horizontal,
    HorizontalMask,
    Vertical,
    VerticalMask,
    Counter,
    Mask,
    Path,
}
#[derive(PartialEq)]
enum Flow {
    Return,
    End,
}

pub(super) fn outline(
    font: &Cff,
    data: &[u8],
    glyph: GlyphId,
    work: &mut Work<'_>,
) -> Result<Path> {
    Ok(load(font, data, glyph, 0, work)?.0)
}

/// The glyph's width, from the whole interpreted glyph.
pub(super) fn advance(font: &Cff, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<f64> {
    Ok(load(font, data, glyph, 0, work)?.1)
}

fn load(
    font: &Cff,
    data: &[u8],
    glyph: GlyphId,
    depth: usize,
    work: &mut Work<'_>,
) -> Result<(Path, f64)> {
    let charstring = font
        .glyphs
        .get(data, u32_to_usize(glyph.0))
        .ok_or_else(|| syntax(Position::Unknown, "glyph index outside CFF"))?;
    let at = charstring.position();
    work.depth(depth, usize::MAX, at)?;
    let private = font
        .private(glyph)
        .ok_or_else(|| syntax(at, "glyph without a CFF Private DICT"))?;
    let mut state = State {
        // Type 2 charstrings hold at most 48 operands (CFF2: 513).
        stack: Vec::with_capacity(48),
        transient: [0.0; TRANSIENT_SLOTS],
        path: PathBuilder::new(),
        point: Point::default(),
        open: false,
        width: None,
        hints: 0,
        hint_phase: HintPhase::Horizontal,
        random: private.random_seed,
        seac: None,
        // Inherited selection agrees with skrifa 0.46.2; CFF2's primary
        // text remains pending review (see the requirement registry).
        vsindex: private.vsindex,
        stopped: false,
    };
    run(font, private, data, charstring, 0, &mut state, work)?;
    state.close(work, at)?;
    if let Some([dx, dy, base, accent]) = state.seac {
        if depth != 0 {
            return Err(syntax(at, "nested seac composite"));
        }
        if font.cids.is_some() {
            return Err(syntax(at, "seac in CID-keyed CFF"));
        }
        let mut component = |code: f64, transform: Transform| -> Result<()> {
            let code = u8::try_from(dict::integer(code, at)?)
                .map_err(|_| syntax(at, "invalid seac character code"))?;
            let name = strings::standard(code)
                .ok_or_else(|| syntax(at, "undefined seac character code"))?;
            let glyph = font
                .name(name.as_bytes())
                .ok_or_else(|| syntax(at, "missing seac component"))?;
            let (path, _) = load(font, data, glyph, depth + 1, work)?;
            append(&mut state.path, &path, transform, work, at)
        };
        component(base, Transform::IDENTITY)?;
        component(accent, Transform::translate(dx, dy))?;
    }
    let path = state.path.finish().transformed(private.matrix);
    let width = state.width.unwrap_or(private.default_width) * private.matrix.a;
    if !width.is_finite() {
        return Err(syntax(at, "nonfinite charstring width"));
    }
    Ok((path, width))
}
fn append(
    builder: &mut PathBuilder,
    path: &Path,
    transform: Transform,
    work: &mut Work<'_>,
    at: Position,
) -> Result<()> {
    for &segment in path.segments() {
        work.segments(1, at)?;
        match segment {
            Segment::MoveTo(p) => {
                builder.move_to(transform.apply(p));
            }
            Segment::LineTo(p) => {
                builder
                    .line_to(transform.apply(p))
                    .ok_or_else(|| syntax(at, "invalid seac path"))?;
            }
            Segment::CubicTo(a, b, c) => {
                builder
                    .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                    .ok_or_else(|| syntax(at, "invalid seac path"))?;
            }
            Segment::Close => {
                builder.close();
            }
        }
    }
    Ok(())
}
impl State {
    fn pop(&mut self, at: Position) -> Result<f64> {
        self.stack
            .pop()
            .ok_or_else(|| syntax(at, "charstring stack underflow"))
    }
    fn count(&self, n: usize, at: Position) -> Result<()> {
        if self.stack.len() == n {
            Ok(())
        } else {
            Err(syntax(at, "invalid charstring operand count"))
        }
    }
    fn width(&mut self, private: &Private, present: bool, at: Position) -> Result<()> {
        if self.width.is_none() {
            self.width = Some(if present {
                if self.stack.is_empty() {
                    return Err(syntax(at, "missing charstring width"));
                }
                self.stack.remove(0) + private.nominal_width
            } else {
                private.default_width
            });
        }
        Ok(())
    }
    fn close(&mut self, work: &mut Work<'_>, at: Position) -> Result<()> {
        if self.open {
            work.segments(1, at)?;
            self.path.close();
            self.open = false;
        }
        Ok(())
    }
    fn point(&mut self, dx: f64, dy: f64, at: Position) -> Result<Point> {
        self.point.x += dx;
        self.point.y += dy;
        if !self.point.is_finite() {
            return Err(syntax(at, "nonfinite charstring coordinate"));
        }
        Ok(self.point)
    }
    fn moveto(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: Position) -> Result<()> {
        self.close(work, at)?;
        let p = self.point(dx, dy, at)?;
        work.segments(1, at)?;
        self.path.move_to(p);
        self.open = true;
        Ok(())
    }
    fn line(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: Position) -> Result<()> {
        if !self.open {
            return Err(syntax(at, "charstring line before moveto"));
        }
        let p = self.point(dx, dy, at)?;
        work.segments(1, at)?;
        self.path
            .line_to(p)
            .ok_or_else(|| syntax(at, "charstring line without current point"))?;
        Ok(())
    }
    fn curve(&mut self, deltas: &[f64], work: &mut Work<'_>, at: Position) -> Result<()> {
        let (true, &[dxa, dya, dxb, dyb, dxc, dyc]) = (self.open, deltas) else {
            return Err(syntax(at, "invalid charstring curve"));
        };
        let a = self.point(dxa, dya, at)?;
        let b = self.point(dxb, dyb, at)?;
        let c = self.point(dxc, dyc, at)?;
        work.segments(1, at)?;
        self.path
            .cubic_to(a, b, c)
            .ok_or_else(|| syntax(at, "charstring curve without current point"))?;
        Ok(())
    }
}

/// Checks the format ceilings of a charstring about to run: its nesting
/// and, for CFF, its length.
fn enter(version: Version, r: &Cursor<'_>, depth: usize, work: &Work<'_>) -> Result<()> {
    let nesting = match version {
        Version::Cff1 => MAX_SUBR_DEPTH,
        Version::Cff2 => usize::MAX,
    };
    work.depth(depth, nesting, r.position())?;
    if version == Version::Cff1 && r.remaining() > MAX_CHARSTRING_BYTES {
        return Err(r.error("Type 2 charstring exceeds 65,535 bytes"));
    }
    Ok(())
}

/// Whether `op` exists in `version`: CFF2 replaces `return` and `endchar`
/// with implicit ends, and adds `vsindex` and `blend`.
fn available(op: Operator, version: Version) -> bool {
    match op {
        Operator::Return | Operator::Endchar => version == Version::Cff1,
        Operator::Vsindex | Operator::Blend => version == Version::Cff2,
        _ => true,
    }
}

fn run(
    font: &Cff,
    private: &Private,
    data: &[u8],
    mut r: Cursor<'_>,
    depth: usize,
    state: &mut State,
    work: &mut Work<'_>,
) -> Result<Flow> {
    let version = font.version;
    enter(version, &r, depth, work)?;
    let ceiling = match version {
        Version::Cff1 => MAX_OPERANDS,
        Version::Cff2 => MAX_CFF2_OPERANDS,
    };
    while !r.at_end() {
        let at = r.position();
        work.work(1, at)?;
        let byte = r.u8()?;
        if byte == SHORT_INT || byte >= 32 {
            if state.stack.len() == ceiling {
                return Err(syntax(at, "operand stack exceeds the format limit"));
            }
            state.stack.push(number(byte, &mut r)?);
            continue;
        }
        let op = Operator::try_from(byte)
            .ok()
            .filter(|&op| available(op, version))
            .ok_or_else(|| syntax(at, "invalid Type 2 charstring operator"))?;
        match op {
            Operator::Hstem
            | Operator::Vstem
            | Operator::Hstemhm
            | Operator::Vstemhm
            | Operator::Hintmask
            | Operator::Cntrmask => hints(version, private, op, &mut r, state, at)?,
            Operator::Vmoveto | Operator::Rmoveto | Operator::Hmoveto => {
                move_operator(version, private, op, state, work, at)?;
            }
            Operator::Callsubr | Operator::Callgsubr => {
                let subrs = if op == Operator::Callsubr {
                    &private.subrs
                } else {
                    &font.global
                };
                let index = dict::integer(state.pop(at)? + bias(subrs.len()), at)?;
                let subr = subrs
                    .get(data, index)
                    .ok_or_else(|| syntax(at, "charstring subroutine index out of range"))?;
                if run(font, private, data, subr, depth + 1, state, work)? == Flow::End {
                    if !state.stopped && !r.at_end() {
                        return Err(r.error("data after charstring end"));
                    }
                    return Ok(Flow::End);
                }
            }
            Operator::Return => {
                if depth == 0 {
                    return Err(syntax(at, "return outside charstring subroutine"));
                }
                return Ok(Flow::Return);
            }
            Operator::Escape => {
                if escape(r.u8()?, state, work, at)? {
                    return Ok(Flow::End);
                }
            }
            Operator::Endchar => return finish(private, state, &r, at),
            Operator::Vsindex => {
                state.count(1, at)?;
                state.vsindex = dict::integer(state.pop(at)?, at)?;
                if state.vsindex >= font.variations.len() {
                    return Err(syntax(at, "CFF2 vsindex out of range"));
                }
            }
            Operator::Blend => blend(font, state, at)?,
            _ => draw(op, state, work, at)?,
        }
        if state.stack.len() > ceiling {
            return Err(syntax(at, "operand stack exceeds the format limit"));
        }
        // Finite operands are an invariant: arithmetic writes only the top
        // operand, and blend validates every value it writes in place.
        if state.stack.last().is_some_and(|n| !n.is_finite()) {
            return Err(syntax(at, "nonfinite charstring operand"));
        }
    }
    end_of_program(version, depth, state, &r)
}

fn end_of_program(version: Version, depth: usize, state: &State, r: &Cursor<'_>) -> Result<Flow> {
    match version {
        Version::Cff2 if depth == 0 && !state.stack.is_empty() => {
            Err(r.error("unconsumed CFF2 operands"))
        }
        Version::Cff2 if depth == 0 => Ok(Flow::End),
        // CFF2 subroutines return implicitly at their end.
        Version::Cff2 => Ok(Flow::Return),
        Version::Cff1 => Err(r.error("unterminated charstring")),
    }
}

fn number(byte: u8, r: &mut Cursor<'_>) -> Result<f64> {
    if byte == SHORT_INT {
        return Ok(f64::from(r.i16()?));
    }
    match dict::shared_number(byte, r)? {
        Some(n) => Ok(n),
        None if byte == FIXED => r.fixed(),
        None => Err(r.error("invalid Type 2 charstring number")),
    }
}

/// The subroutine number bias for a subroutine INDEX of `count` (TN 5177
/// 4.7).
fn bias(count: usize) -> f64 {
    if count < 1240 {
        107.0
    } else if count < 33900 {
        1131.0
    } else {
        32768.0
    }
}

fn blend(font: &Cff, state: &mut State, at: Position) -> Result<()> {
    let count = dict::integer(state.pop(at)?, at)?;
    let coefficients = font
        .variations
        .get(state.vsindex)
        .ok_or_else(|| syntax(at, "missing CFF2 variation data"))?;
    let size = count
        .checked_mul(coefficients.len() + 1)
        .ok_or_else(|| limit(at, "CFF2 blend count overflow"))?;
    if count == 0 || size > state.stack.len() {
        return Err(syntax(at, "invalid CFF2 blend operands"));
    }
    let start = state.stack.len() - size;
    for i in 0..count {
        let mut value = state.stack[start + i];
        for (j, coefficient) in coefficients.iter().enumerate() {
            value += coefficient * state.stack[start + count + i * coefficients.len() + j];
        }
        if !value.is_finite() {
            return Err(syntax(at, "nonfinite charstring blend result"));
        }
        state.stack[start + i] = value;
    }
    state.stack.truncate(start + count);
    Ok(())
}

/// The path operators (TN 5177 4.1), consuming every operand; the stack's
/// storage returns, empty, for the next operands.
fn draw(op: Operator, s: &mut State, w: &mut Work<'_>, at: Position) -> Result<()> {
    let mut args = std::mem::take(&mut s.stack);
    let result = draw_operands(op, &args, s, w, at);
    args.clear();
    s.stack = args;
    result
}

fn draw_operands(
    op: Operator,
    args: &[f64],
    s: &mut State,
    w: &mut Work<'_>,
    at: Position,
) -> Result<()> {
    let n = args.len();
    match op {
        Operator::Rlineto if n >= 2 && n.is_multiple_of(2) => {
            for a in args.as_chunks::<2>().0 {
                s.line(a[0], a[1], w, at)?;
            }
        }
        Operator::Hlineto | Operator::Vlineto if n > 0 => {
            let mut horizontal = op == Operator::Hlineto;
            for &a in args {
                s.line(
                    if horizontal { a } else { 0.0 },
                    if horizontal { 0.0 } else { a },
                    w,
                    at,
                )?;
                horizontal = !horizontal;
            }
        }
        Operator::Rrcurveto if n >= 6 && n.is_multiple_of(6) => {
            for a in args.as_chunks::<6>().0 {
                s.curve(a, w, at)?;
            }
        }
        Operator::Rcurveline if n >= 8 && (n - 2).is_multiple_of(6) => {
            for a in args[..n - 2].as_chunks::<6>().0 {
                s.curve(a, w, at)?;
            }
            s.line(args[n - 2], args[n - 1], w, at)?;
        }
        Operator::Rlinecurve if n >= 8 && (n - 6).is_multiple_of(2) => {
            for a in args[..n - 6].as_chunks::<2>().0 {
                s.line(a[0], a[1], w, at)?;
            }
            s.curve(&args[n - 6..], w, at)?;
        }
        Operator::Vvcurveto | Operator::Hhcurveto if n >= 4 && matches!(n % 4, 0 | 1) => {
            let mut offset = n % 4;
            let mut extra = if offset == 1 { args[0] } else { 0.0 };
            while offset < n {
                let a = &args[offset..offset + 4];
                let deltas = if op == Operator::Vvcurveto {
                    [extra, a[0], a[1], a[2], 0.0, a[3]]
                } else {
                    [a[0], extra, a[1], a[2], a[3], 0.0]
                };
                s.curve(&deltas, w, at)?;
                extra = 0.0;
                offset += 4;
            }
        }
        Operator::Vhcurveto | Operator::Hvcurveto if n >= 4 && matches!(n % 4, 0 | 1) => {
            let mut offset = 0;
            let mut horizontal = op == Operator::Hvcurveto;
            while n - offset >= 4 {
                let a = &args[offset..offset + 4];
                let extra = if n - offset == 5 {
                    args[offset + 4]
                } else {
                    0.0
                };
                let deltas = if horizontal {
                    [a[0], 0.0, a[1], a[2], extra, a[3]]
                } else {
                    [0.0, a[0], a[1], a[2], a[3], extra]
                };
                s.curve(&deltas, w, at)?;
                offset += 4;
                horizontal = !horizontal;
            }
        }
        _ => return Err(syntax(at, "invalid charstring drawing operands")),
    }
    Ok(())
}

/// An escaped operator; `true` if it ended the glyph. An unknown one ends
/// the glyph as a reported repair: TN 5177 gives it no semantics.
fn escape(code: u8, s: &mut State, w: &mut Work<'_>, at: Position) -> Result<bool> {
    let Ok(op) = Escape::try_from(code) else {
        w.repair(
            repairs::CHARSTRING_OPERATOR,
            "unknown escaped Type 2 operator ends glyph",
            at,
        )?;
        s.stack.clear();
        s.stopped = true;
        return Ok(true);
    };
    match op {
        Escape::Dotsection => {} // TN 5177 Appendix C: deprecated, a no-op.
        Escape::And
        | Escape::Or
        | Escape::Add
        | Escape::Sub
        | Escape::Div
        | Escape::Eq
        | Escape::Mul => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            let value = match op {
                Escape::And => f64::from(a != 0.0 && b != 0.0),
                Escape::Or => f64::from(a != 0.0 || b != 0.0),
                Escape::Add => a + b,
                Escape::Sub => a - b,
                Escape::Div => a / b,
                Escape::Eq => f64::from(a == b),
                _ => a * b,
            };
            s.stack.push(value);
        }
        Escape::Not | Escape::Abs | Escape::Neg | Escape::Sqrt => {
            let a = s.pop(at)?;
            s.stack.push(match op {
                Escape::Not => f64::from(a == 0.0),
                Escape::Abs => a.abs(),
                Escape::Neg => -a,
                _ => a.sqrt(),
            });
        }
        Escape::Drop => {
            s.pop(at)?;
        }
        Escape::Put => {
            let index = dict::integer(s.pop(at)?, at)?;
            let value = s.pop(at)?;
            *s.transient
                .get_mut(index)
                .ok_or_else(|| syntax(at, "invalid transient array index"))? = value;
        }
        Escape::Get => {
            let index = dict::integer(s.pop(at)?, at)?;
            s.stack.push(
                *s.transient
                    .get(index)
                    .ok_or_else(|| syntax(at, "invalid transient array index"))?,
            );
        }
        Escape::Ifelse => {
            let v2 = s.pop(at)?;
            let v1 = s.pop(at)?;
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.push(if v1 <= v2 { a } else { b });
        }
        Escape::Random => {
            s.random = s
                .random
                .wrapping_mul(RANDOM_MULTIPLIER)
                .wrapping_add(RANDOM_INCREMENT);
            s.stack.push((f64::from(s.random) + 1.0) / RANDOM_RANGE);
        }
        Escape::Dup => {
            let a = s.pop(at)?;
            s.stack.extend([a, a]);
        }
        Escape::Exch => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.extend([b, a]);
        }
        Escape::Index => index(s, at)?,
        Escape::Roll => {
            let shift = s.pop(at)?;
            let count_value = s.pop(at)?;
            let count = dict::integer(count_value, at)?;
            if count > s.stack.len() || shift.fract() != 0.0 || shift.abs() > f64::from(i32::MAX) {
                return Err(syntax(at, "invalid charstring roll"));
            }
            if count == 0 {
                return Ok(false);
            }
            let rotation = dict::integer(shift.rem_euclid(count_value), at)?;
            let start = s.stack.len() - count;
            s.stack[start..].rotate_right(rotation);
        }
        Escape::Hflex | Escape::Flex | Escape::Hflex1 | Escape::Flex1 => flex(op, s, w, at)?,
    }
    Ok(false)
}

fn flex(op: Escape, s: &mut State, w: &mut Work<'_>, at: Position) -> Result<()> {
    let mut a = std::mem::take(&mut s.stack);
    let curves = flex_curves(op, &a, at);
    a.clear();
    s.stack = a;
    let (first, second) = curves?;
    s.curve(&first, w, at)?;
    s.curve(&second, w, at)
}

/// The two curves of a flex operator (TN 5177 4.1 and Appendix C).
fn flex_curves(op: Escape, a: &[f64], at: Position) -> Result<([f64; 6], [f64; 6])> {
    Ok(match (op, a) {
        (Escape::Hflex, [a, b, c, d, e, f, g]) => {
            ([*a, 0.0, *b, *c, *d, 0.0], [*e, 0.0, *f, -c, *g, 0.0])
        }
        (Escape::Flex, [a, b, c, d, e, f, g, h, i, j, k, l, _]) => {
            ([*a, *b, *c, *d, *e, *f], [*g, *h, *i, *j, *k, *l])
        }
        (Escape::Hflex1, [a, b, c, d, e, f, g, h, i]) => (
            [*a, *b, *c, *d, *e, 0.0],
            [*f, 0.0, *g, *h, *i, -(b + d + h)],
        ),
        (Escape::Flex1, [a, b, c, d, e, f, g, h, i, j, k]) => {
            let dx = a + c + e + g + i;
            let dy = b + d + f + h + j;
            (
                [*a, *b, *c, *d, *e, *f],
                [
                    *g,
                    *h,
                    *i,
                    *j,
                    if dx.abs() > dy.abs() { *k } else { -dx },
                    if dx.abs() > dy.abs() { -dy } else { *k },
                ],
            )
        }
        _ => return Err(syntax(at, "invalid flex operands")),
    })
}

/// Hint declarations and masks (TN 5177 4.3). Hints are validated and
/// skipped: outlines are unhinted.
fn hints(
    version: Version,
    private: &Private,
    op: Operator,
    r: &mut Cursor<'_>,
    state: &mut State,
    at: Position,
) -> Result<()> {
    if version == Version::Cff2 {
        state.width = Some(0.0);
    }
    state.width(private, state.stack.len() % 2 == 1, at)?;
    if !state.stack.len().is_multiple_of(2) {
        return Err(syntax(at, "odd charstring hint operands"));
    }
    let mask = matches!(op, Operator::Hintmask | Operator::Cntrmask);
    let phase = match op {
        Operator::Hstem => HintPhase::Horizontal,
        Operator::Hstemhm => HintPhase::HorizontalMask,
        Operator::Vstem => HintPhase::Vertical,
        Operator::Vstemhm => HintPhase::VerticalMask,
        Operator::Cntrmask => HintPhase::Counter,
        _ => HintPhase::Mask,
    };
    if version == Version::Cff1 {
        if (!mask || op == Operator::Cntrmask) && phase < state.hint_phase
            || state.hint_phase >= HintPhase::Counter && !state.stack.is_empty()
        {
            return Err(syntax(at, "charstring hints out of order"));
        }
        state.hint_phase = state.hint_phase.max(phase);
    }
    if !mask && state.stack.is_empty() {
        return Err(syntax(at, "charstring stem operator without operands"));
    }
    state.hints += state.stack.len() / 2;
    if state.hints > MAX_STEM_HINTS {
        return Err(syntax(at, "charstring stems exceed 96"));
    }
    state.stack.clear();
    if mask {
        if state.hints == 0 {
            return Err(syntax(at, "charstring mask without stems"));
        }
        let bytes = r.take(state.hints.div_ceil(8))?;
        let unused = (8 - state.hints % 8) % 8;
        if bytes
            .last()
            .is_some_and(|last| last & ((1 << unused) - 1) != 0)
        {
            return Err(syntax(
                Position::Buffer(here(r) - 1),
                "nonzero charstring mask padding",
            ));
        }
    }
    Ok(())
}

fn move_operator(
    version: Version,
    private: &Private,
    op: Operator,
    state: &mut State,
    work: &mut Work<'_>,
    at: Position,
) -> Result<()> {
    let n = if op == Operator::Rmoveto { 2 } else { 1 };
    if version == Version::Cff2 {
        state.width = Some(0.0);
    }
    state.width(private, state.stack.len() == n + 1, at)?;
    state.count(n, at)?;
    let (dx, dy) = match (op, state.stack.as_slice()) {
        (Operator::Vmoveto, &[dy]) => (0.0, dy),
        (Operator::Hmoveto, &[dx]) => (dx, 0.0),
        (_, &[dx, dy]) => (dx, dy),
        _ => return Err(syntax(at, "invalid charstring moveto operands")),
    };
    state.stack.clear();
    state.hint_phase = HintPhase::Path;
    state.moveto(dx, dy, work, at)?;
    Ok(())
}

fn finish(private: &Private, state: &mut State, r: &Cursor<'_>, at: Position) -> Result<Flow> {
    state.width(private, matches!(state.stack.len(), 1 | 5), at)?;
    match state.stack.as_slice() {
        [] => {}
        [dx, dy, base, accent] => state.seac = Some([*dx, *dy, *base, *accent]),
        _ => return Err(syntax(at, "invalid endchar operands")),
    }
    state.stack.clear();
    if !r.at_end() {
        return Err(r.error("data after charstring endchar"));
    }
    Ok(Flow::End)
}

fn index(s: &mut State, at: Position) -> Result<()> {
    let index = dict::integer(s.pop(at)?.max(0.0), at)?;
    let value = *s
        .stack
        .iter()
        .rev()
        .nth(index)
        .ok_or_else(|| syntax(at, "charstring index outside stack"))?;
    s.stack.push(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bias, number};
    use crate::read::cursor;

    #[test]
    fn subroutine_bias_follows_the_index_size() {
        assert_eq!(bias(0), 107.0);
        assert_eq!(bias(1239), 107.0);
        assert_eq!(bias(1240), 1131.0);
        assert_eq!(bias(33899), 1131.0);
        assert_eq!(bias(33900), 32768.0);
    }

    #[test]
    fn short_integers_and_fixed_point_numbers() {
        assert_eq!(number(28, &mut cursor(&[0x01, 0x00])).unwrap(), 256.0);
        assert_eq!(number(255, &mut cursor(&[0, 1, 0x80, 0])).unwrap(), 1.5);
        assert_eq!(number(139, &mut cursor(&[])).unwrap(), 0.0);
    }
}
