//! Bounded, unhinted Type 2 charstrings (Adobe Technical Note 5177).
use super::{Cff, Private, Version, strings};
use crate::budget::Work;
use crate::charstring::{Flow, Operands, integer, shared_number};
use crate::outline::Pen;
use crate::read::{here, syntax};
use crate::{GlyphId, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use pagura_raster::{Path, Point, Transform};

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

/// What the interpreter is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The whole outline, including seac components.
    Outline,
    /// Only the width: interpretation stops once it is known.
    Width,
}

/// One glyph's interpreted outline and advance width in glyph space.
struct Glyph {
    path: Path,
    width: f64,
}

/// The base and accent of a `seac`-style endchar (TN 5177 Appendix C).
#[derive(Debug, Clone, Copy)]
struct Seac {
    dx: f64,
    dy: f64,
    base: f64,
    accent: f64,
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

/// The pen state that drawing operators move.
struct Drawing {
    pen: Pen,
    point: Point,
}

struct State<const N: usize> {
    operands: Operands<N>,
    transient: [f64; TRANSIENT_SLOTS],
    drawing: Drawing,
    width: Option<f64>,
    hints: usize,
    hint_phase: HintPhase,
    seac: Option<Seac>,
    vsindex: usize,
    random: u32,
}

/// One outline or width request: the font, its bytes and budgets.
struct Run<'r, 'w> {
    font: &'r Cff,
    data: &'r [u8],
    work: &'r mut Work<'w>,
    mode: Mode,
    /// A repair or a width request ended the glyph before its end.
    stopped: bool,
}

pub(super) fn outline(
    font: &Cff,
    data: &[u8],
    glyph: GlyphId,
    work: &mut Work<'_>,
) -> Result<Path> {
    run(font, data, glyph, Mode::Outline, work).map(|g| g.path)
}

pub(super) fn advance(font: &Cff, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<f64> {
    run(font, data, glyph, Mode::Width, work).map(|g| g.width)
}

fn run(font: &Cff, data: &[u8], glyph: GlyphId, mode: Mode, work: &mut Work<'_>) -> Result<Glyph> {
    Run {
        font,
        data,
        work,
        mode,
        stopped: false,
    }
    .load(glyph, 0)
}

impl Run<'_, '_> {
    fn load(&mut self, glyph: GlyphId, depth: usize) -> Result<Glyph> {
        let index = u32_to_usize(glyph.0);
        let charstring = self
            .font
            .glyphs
            .get(self.data, index)
            .ok_or_else(|| syntax(Position::Unknown, "glyph index outside CFF"))?;
        let at = charstring.position();
        self.work.depth(depth, usize::MAX, at)?;
        let private = self
            .font
            .private(glyph)
            .ok_or_else(|| syntax(at, "glyph without a CFF Private DICT"))?;
        match self.font.version {
            Version::Cff1 => self.interpret::<MAX_OPERANDS>(depth, charstring, private),
            Version::Cff2 => self.interpret::<MAX_CFF2_OPERANDS>(depth, charstring, private),
        }
    }

    /// Interprets a glyph's charstring with an operand stack of `N`.
    fn interpret<const N: usize>(
        &mut self,
        depth: usize,
        charstring: Cursor<'_>,
        private: &Private,
    ) -> Result<Glyph> {
        let at = charstring.position();
        let mut state = State::<N> {
            operands: Operands::new(),
            transient: [0.0; TRANSIENT_SLOTS],
            drawing: Drawing {
                pen: Pen::new(),
                point: Point::default(),
            },
            width: None,
            hints: 0,
            hint_phase: HintPhase::Horizontal,
            random: private.random_seed,
            seac: None,
            // Inherited selection agrees with skrifa 0.46.2; CFF2's primary
            // text remains pending review (see the requirement registry).
            vsindex: private.vsindex,
        };
        self.charstring(private, charstring, 0, &mut state)?;
        let width = state.width.unwrap_or(private.default_width) * private.matrix.a;
        if !width.is_finite() {
            return Err(syntax(at, "nonfinite charstring width"));
        }
        if self.mode == Mode::Width {
            return Ok(Glyph {
                path: Path::default(),
                width,
            });
        }
        let mut pen = state.drawing.pen;
        pen.close(self.work, at)?;
        if let Some(seac) = state.seac {
            if depth != 0 {
                return Err(syntax(at, "nested seac composite"));
            }
            if self.font.cids.is_some() {
                return Err(syntax(at, "seac in CID-keyed CFF"));
            }
            for (code, transform) in [
                (seac.base, Transform::IDENTITY),
                (seac.accent, Transform::translate(seac.dx, seac.dy)),
            ] {
                let code = u8::try_from(integer(code, at)?)
                    .map_err(|_| syntax(at, "invalid seac character code"))?;
                let name = strings::standard(code)
                    .ok_or_else(|| syntax(at, "undefined seac character code"))?;
                let component = self
                    .font
                    .name(name.as_bytes())
                    .ok_or_else(|| syntax(at, "missing seac component"))?;
                let component = self.load(component, depth + 1)?;
                pen.append(self.work, &component.path, transform, at)?;
            }
        }
        Ok(Glyph {
            path: pen.finish(private.matrix, at)?,
            width,
        })
    }

    fn charstring<const N: usize>(
        &mut self,
        private: &Private,
        mut r: Cursor<'_>,
        depth: usize,
        s: &mut State<N>,
    ) -> Result<Flow> {
        let version = self.font.version;
        let start = r.position();
        self.work.depth(
            depth,
            match version {
                Version::Cff1 => MAX_SUBR_DEPTH,
                Version::Cff2 => usize::MAX,
            },
            start,
        )?;
        if version == Version::Cff1 && r.remaining() > MAX_CHARSTRING_BYTES {
            return Err(syntax(start, "Type 2 charstring exceeds 65,535 bytes"));
        }
        while !r.at_end() {
            let at = r.position();
            self.work.work(1, at)?;
            let byte = r.u8()?;
            if byte == SHORT_INT || byte >= 32 {
                s.operands.push(number(byte, &mut r)?, at)?;
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
                | Operator::Cntrmask => hints(version, private, op, &mut r, s, at)?,
                Operator::Vmoveto | Operator::Rmoveto | Operator::Hmoveto => {
                    self.move_to(private, op, s, at)?;
                }
                Operator::Callsubr | Operator::Callgsubr => {
                    if let Some(flow) = self.call(private, op, &r, depth, s, at)? {
                        return Ok(flow);
                    }
                }
                Operator::Return => {
                    if depth == 0 {
                        return Err(syntax(at, "return outside charstring subroutine"));
                    }
                    return Ok(Flow::Return);
                }
                Operator::Escape => {
                    let code = r.u8()?;
                    let Ok(escape) = Escape::try_from(code) else {
                        return self.unknown_escape(s, at);
                    };
                    self.escape(escape, s, at)?;
                }
                Operator::Endchar => return end_char(private, s, &r, at),
                Operator::Vsindex | Operator::Blend => self.variation(op, s, at)?,
                _ => {
                    s.drawing.draw(op, s.operands.as_slice(), self.work, at)?;
                    s.operands.clear();
                }
            }
            if self.mode == Mode::Width && s.width.is_some() {
                self.stopped = true;
                return Ok(Flow::End);
            }
        }
        match version {
            Version::Cff2 if depth == 0 && !s.operands.is_empty() => {
                Err(r.error("unconsumed CFF2 operands"))
            }
            Version::Cff2 if depth == 0 => Ok(Flow::End),
            // CFF2 subroutines return implicitly at their end.
            Version::Cff2 => Ok(Flow::Return),
            Version::Cff1 => Err(r.error("unterminated charstring")),
        }
    }

    /// `callsubr` or `callgsubr`: `Some(End)` if the subroutine ended the
    /// glyph, so the caller must end too.
    fn call<const N: usize>(
        &mut self,
        private: &Private,
        op: Operator,
        r: &Cursor<'_>,
        depth: usize,
        s: &mut State<N>,
        at: Position,
    ) -> Result<Option<Flow>> {
        let subrs = if op == Operator::Callsubr {
            &private.subrs
        } else {
            &self.font.global
        };
        let index = integer(s.operands.pop(at)? + bias(subrs.len()), at)?;
        let subr = subrs
            .get(self.data, index)
            .ok_or_else(|| syntax(at, "charstring subroutine index out of range"))?;
        if self.charstring(private, subr, depth + 1, s)? == Flow::Return {
            return Ok(None);
        }
        if !self.stopped && !r.at_end() {
            return Err(r.error("data after charstring end"));
        }
        Ok(Some(Flow::End))
    }

    /// An unknown escaped operator ends the glyph as a reported repair:
    /// TN 5177 gives it no semantics.
    fn unknown_escape<const N: usize>(&mut self, s: &mut State<N>, at: Position) -> Result<Flow> {
        self.work.repair(
            repairs::CHARSTRING_OPERATOR,
            "unknown escaped Type 2 operator ends glyph",
            at,
        )?;
        s.operands.clear();
        self.stopped = true;
        Ok(Flow::End)
    }

    /// CFF2 `vsindex` and `blend`.
    fn variation<const N: usize>(
        &self,
        op: Operator,
        s: &mut State<N>,
        at: Position,
    ) -> Result<()> {
        if op == Operator::Vsindex {
            let &[index] = s.operands.exactly(1, at)? else {
                return Err(syntax(at, "invalid CFF2 vsindex operands"));
            };
            s.operands.clear();
            s.vsindex = integer(index, at)?;
            if s.vsindex >= self.font.variations.len() {
                return Err(syntax(at, "CFF2 vsindex out of range"));
            }
            return Ok(());
        }
        let count = integer(s.operands.pop(at)?, at)?;
        let coefficients = self
            .font
            .variations
            .get(s.vsindex)
            .ok_or_else(|| syntax(at, "missing CFF2 variation data"))?;
        s.operands.blend(count, coefficients, at)
    }

    fn move_to<const N: usize>(
        &mut self,
        private: &Private,
        op: Operator,
        s: &mut State<N>,
        at: Position,
    ) -> Result<()> {
        let count = if op == Operator::Rmoveto { 2 } else { 1 };
        if self.font.version == Version::Cff2 {
            s.width = Some(0.0);
        }
        s.set_width(private, s.operands.len() == count + 1, at)?;
        let (dx, dy) = match (op, s.operands.exactly(count, at)?) {
            (Operator::Vmoveto, &[dy]) => (0.0, dy),
            (Operator::Hmoveto, &[dx]) => (dx, 0.0),
            (_, &[dx, dy]) => (dx, dy),
            _ => return Err(syntax(at, "invalid charstring moveto operands")),
        };
        s.operands.clear();
        s.hint_phase = HintPhase::Path;
        let to = s.drawing.advance(dx, dy);
        s.drawing.pen.move_to(self.work, to, at)
    }

    fn escape<const N: usize>(&mut self, op: Escape, s: &mut State<N>, at: Position) -> Result<()> {
        let stack = &mut s.operands;
        match op {
            Escape::Dotsection => {} // TN 5177 Appendix C: deprecated, a no-op.
            Escape::And
            | Escape::Or
            | Escape::Add
            | Escape::Sub
            | Escape::Div
            | Escape::Eq
            | Escape::Mul => {
                let b = stack.pop(at)?;
                let a = stack.pop(at)?;
                stack.push(
                    match op {
                        Escape::And => f64::from(a != 0.0 && b != 0.0),
                        Escape::Or => f64::from(a != 0.0 || b != 0.0),
                        Escape::Add => a + b,
                        Escape::Sub => a - b,
                        Escape::Div => a / b,
                        Escape::Eq => f64::from(a == b),
                        _ => a * b,
                    },
                    at,
                )?;
            }
            Escape::Not | Escape::Abs | Escape::Neg | Escape::Sqrt => {
                let a = stack.pop(at)?;
                stack.push(
                    match op {
                        Escape::Not => f64::from(a == 0.0),
                        Escape::Abs => a.abs(),
                        Escape::Neg => -a,
                        _ => a.sqrt(),
                    },
                    at,
                )?;
            }
            Escape::Drop => {
                stack.pop(at)?;
            }
            Escape::Put => {
                let index = integer(stack.pop(at)?, at)?;
                let value = stack.pop(at)?;
                *s.transient
                    .get_mut(index)
                    .ok_or_else(|| syntax(at, "invalid transient array index"))? = value;
            }
            Escape::Get => {
                let index = integer(stack.pop(at)?, at)?;
                let value = *s
                    .transient
                    .get(index)
                    .ok_or_else(|| syntax(at, "invalid transient array index"))?;
                stack.push(value, at)?;
            }
            Escape::Ifelse => {
                let v2 = stack.pop(at)?;
                let v1 = stack.pop(at)?;
                let b = stack.pop(at)?;
                let a = stack.pop(at)?;
                stack.push(if v1 <= v2 { a } else { b }, at)?;
            }
            Escape::Random => {
                s.random = s
                    .random
                    .wrapping_mul(RANDOM_MULTIPLIER)
                    .wrapping_add(RANDOM_INCREMENT);
                stack.push((f64::from(s.random) + 1.0) / RANDOM_RANGE, at)?;
            }
            Escape::Dup => {
                let a = stack.pop(at)?;
                stack.push(a, at)?;
                stack.push(a, at)?;
            }
            Escape::Exch => {
                let b = stack.pop(at)?;
                let a = stack.pop(at)?;
                stack.push(b, at)?;
                stack.push(a, at)?;
            }
            Escape::Index => {
                let index = integer(stack.pop(at)?.max(0.0), at)?;
                let value = *stack
                    .as_slice()
                    .iter()
                    .rev()
                    .nth(index)
                    .ok_or_else(|| syntax(at, "charstring index outside stack"))?;
                stack.push(value, at)?;
            }
            Escape::Roll => roll(stack, at)?,
            Escape::Hflex | Escape::Flex | Escape::Hflex1 | Escape::Flex1 => {
                let [first, second] = flex(op, stack.as_slice(), at)?;
                stack.clear();
                s.drawing.curve(&first, self.work, at)?;
                s.drawing.curve(&second, self.work, at)?;
            }
        }
        Ok(())
    }
}

impl<const N: usize> State<N> {
    /// Takes the width from the bottom operand if `present`, else the
    /// default width, unless it is already known (TN 5177 3.1).
    fn set_width(&mut self, private: &Private, present: bool, at: Position) -> Result<()> {
        if self.width.is_none() {
            self.width = Some(if present {
                self.operands
                    .shift()
                    .ok_or_else(|| syntax(at, "missing charstring width"))?
                    + private.nominal_width
            } else {
                private.default_width
            });
        }
        Ok(())
    }
}

impl Drawing {
    fn advance(&mut self, dx: f64, dy: f64) -> Point {
        self.point = Point::new(self.point.x + dx, self.point.y + dy);
        self.point
    }

    fn line(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: Position) -> Result<()> {
        if !self.pen.is_open() {
            return Err(syntax(at, "charstring line before moveto"));
        }
        let to = self.advance(dx, dy);
        self.pen.line_to(work, to, at)
    }

    fn curve(
        &mut self,
        &[dxa, dya, dxb, dyb, dxc, dyc]: &[f64; 6],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        if !self.pen.is_open() {
            return Err(syntax(at, "invalid charstring curve"));
        }
        let points = [
            self.advance(dxa, dya),
            self.advance(dxb, dyb),
            self.advance(dxc, dyc),
        ];
        self.pen.cubic_to(work, points, at)
    }

    /// The path operators (TN 5177 4.1), consuming every operand.
    fn draw(
        &mut self,
        op: Operator,
        args: &[f64],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        let invalid = || syntax(at, "invalid charstring drawing operands");
        match op {
            Operator::Rlineto => {
                for &[dx, dy] in chunks::<2>(args).ok_or_else(invalid)? {
                    self.line(dx, dy, work, at)?;
                }
            }
            Operator::Hlineto | Operator::Vlineto => {
                if args.is_empty() {
                    return Err(invalid());
                }
                let mut horizontal = op == Operator::Hlineto;
                for &d in args {
                    let (dx, dy) = if horizontal { (d, 0.0) } else { (0.0, d) };
                    self.line(dx, dy, work, at)?;
                    horizontal = !horizontal;
                }
            }
            Operator::Rrcurveto => {
                for curve in chunks::<6>(args).ok_or_else(invalid)? {
                    self.curve(curve, work, at)?;
                }
            }
            Operator::Rcurveline => {
                let (curves, &[dx, dy]) = args.split_last_chunk::<2>().ok_or_else(invalid)?;
                for curve in chunks::<6>(curves).ok_or_else(invalid)? {
                    self.curve(curve, work, at)?;
                }
                self.line(dx, dy, work, at)?;
            }
            Operator::Rlinecurve => {
                let (lines, curve) = args.split_last_chunk::<6>().ok_or_else(invalid)?;
                for &[dx, dy] in chunks::<2>(lines).ok_or_else(invalid)? {
                    self.line(dx, dy, work, at)?;
                }
                self.curve(curve, work, at)?;
            }
            Operator::Vvcurveto | Operator::Hhcurveto => self.aligned_curves(op, args, work, at)?,
            Operator::Vhcurveto | Operator::Hvcurveto => {
                self.alternating_curves(op, args, work, at)?;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }

    /// `vvcurveto` and `hhcurveto`: curves that start and end along one
    /// axis. An odd leading operand offsets the first curve across it.
    fn aligned_curves(
        &mut self,
        op: Operator,
        args: &[f64],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        let invalid = || syntax(at, "invalid charstring drawing operands");
        let (mut extra, rest) = if args.len() % 4 == 1 {
            let (&first, rest) = args.split_first().ok_or_else(invalid)?;
            (first, rest)
        } else {
            (0.0, args)
        };
        for &[a, b, c, d] in chunks::<4>(rest).ok_or_else(invalid)? {
            let deltas = if op == Operator::Vvcurveto {
                [extra, a, b, c, 0.0, d]
            } else {
                [a, extra, b, c, d, 0.0]
            };
            self.curve(&deltas, work, at)?;
            extra = 0.0;
        }
        Ok(())
    }

    /// `vhcurveto` and `hvcurveto`: curves alternating between the axes. An
    /// odd trailing operand ends the last curve off-axis.
    fn alternating_curves(
        &mut self,
        op: Operator,
        args: &[f64],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        let invalid = || syntax(at, "invalid charstring drawing operands");
        let (rest, last) = if args.len() % 4 == 1 {
            let (&last, rest) = args.split_last().ok_or_else(invalid)?;
            (rest, last)
        } else {
            (args, 0.0)
        };
        let curves = chunks::<4>(rest).ok_or_else(invalid)?;
        let mut horizontal = op == Operator::Hvcurveto;
        for (i, &[a, b, c, d]) in curves.iter().enumerate() {
            let extra = if i + 1 == curves.len() { last } else { 0.0 };
            let deltas = if horizontal {
                [a, 0.0, b, c, extra, d]
            } else {
                [0.0, a, b, c, d, extra]
            };
            self.curve(&deltas, work, at)?;
            horizontal = !horizontal;
        }
        Ok(())
    }
}

/// `args` as one or more groups of `N` operands, with none left over.
fn chunks<const N: usize>(args: &[f64]) -> Option<&[[f64; N]]> {
    match args.as_chunks::<N>() {
        (groups, []) if !groups.is_empty() => Some(groups),
        _ => None,
    }
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

fn number(byte: u8, r: &mut Cursor<'_>) -> Result<f64> {
    if byte == SHORT_INT {
        return Ok(f64::from(r.i16()?));
    }
    match shared_number(byte, r)? {
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

fn roll<const N: usize>(stack: &mut Operands<N>, at: Position) -> Result<()> {
    let shift = stack.pop(at)?;
    let count_value = stack.pop(at)?;
    let count = integer(count_value, at)?;
    if count > stack.len() || shift.fract() != 0.0 || shift.abs() > f64::from(i32::MAX) {
        return Err(syntax(at, "invalid charstring roll"));
    }
    if count == 0 {
        return Ok(());
    }
    let rotation = integer(shift.rem_euclid(count_value), at)?;
    let len = stack.len();
    if let Some(top) = stack.as_mut_slice().get_mut(len - count..) {
        top.rotate_right(rotation);
    }
    Ok(())
}

/// The two curves of a flex operator (TN 5177 4.1 and Appendix C).
fn flex(op: Escape, args: &[f64], at: Position) -> Result<[[f64; 6]; 2]> {
    Ok(match (op, args) {
        (Escape::Hflex, &[a, b, c, d, e, f, g]) => {
            [[a, 0.0, b, c, d, 0.0], [e, 0.0, f, -c, g, 0.0]]
        }
        (Escape::Flex, &[a, b, c, d, e, f, g, h, i, j, k, l, _depth]) => {
            [[a, b, c, d, e, f], [g, h, i, j, k, l]]
        }
        (Escape::Hflex1, &[a, b, c, d, e, f, g, h, i]) => {
            [[a, b, c, d, e, 0.0], [f, 0.0, g, h, i, -(b + d + h)]]
        }
        (Escape::Flex1, &[a, b, c, d, e, f, g, h, i, j, k]) => {
            let dx = a + c + e + g + i;
            let dy = b + d + f + h + j;
            let last = if dx.abs() > dy.abs() {
                [k, -dy]
            } else {
                [-dx, k]
            };
            [[a, b, c, d, e, f], [g, h, i, j, last[0], last[1]]]
        }
        _ => return Err(syntax(at, "invalid flex operands")),
    })
}

/// Hint declarations and masks (TN 5177 4.3). Hints are validated and
/// skipped: outlines are unhinted.
fn hints<const N: usize>(
    version: Version,
    private: &Private,
    op: Operator,
    r: &mut Cursor<'_>,
    s: &mut State<N>,
    at: Position,
) -> Result<()> {
    if version == Version::Cff2 {
        s.width = Some(0.0);
    }
    s.set_width(private, s.operands.len() % 2 == 1, at)?;
    if !s.operands.len().is_multiple_of(2) {
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
        if (!mask || op == Operator::Cntrmask) && phase < s.hint_phase
            || s.hint_phase >= HintPhase::Counter && !s.operands.is_empty()
        {
            return Err(syntax(at, "charstring hints out of order"));
        }
        s.hint_phase = s.hint_phase.max(phase);
    }
    if !mask && s.operands.is_empty() {
        return Err(syntax(at, "charstring stem operator without operands"));
    }
    s.hints += s.operands.len() / 2;
    if s.hints > MAX_STEM_HINTS {
        return Err(syntax(at, "charstring stems exceed 96"));
    }
    s.operands.clear();
    if mask {
        if s.hints == 0 {
            return Err(syntax(at, "charstring mask without stems"));
        }
        let bytes = r.take(s.hints.div_ceil(8))?;
        let unused = (8 - s.hints % 8) % 8;
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

fn end_char<const N: usize>(
    private: &Private,
    s: &mut State<N>,
    r: &Cursor<'_>,
    at: Position,
) -> Result<Flow> {
    s.set_width(private, matches!(s.operands.len(), 1 | 5), at)?;
    match *s.operands.as_slice() {
        [] => {}
        [dx, dy, base, accent] => {
            s.seac = Some(Seac {
                dx,
                dy,
                base,
                accent,
            });
        }
        _ => return Err(syntax(at, "invalid endchar operands")),
    }
    s.operands.clear();
    if !r.at_end() {
        return Err(r.error("data after charstring endchar"));
    }
    Ok(Flow::End)
}

#[cfg(test)]
mod tests {
    use super::{Drawing, Escape, Operator, bias, flex, number};
    use crate::budget::{FontBudget, Work};
    use crate::outline::Pen;
    use crate::read::cursor;
    use pagura_core::{Diagnostics, Limits, Position, Recovery};
    use pagura_raster::Point;

    const AT: Position = Position::Buffer(1);

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

    #[test]
    fn drawing_operators_check_operand_counts_and_alternate_axes() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let mut drawing = Drawing {
            pen: Pen::new(),
            point: Point::default(),
        };
        assert!(
            drawing
                .draw(Operator::Rlineto, &[1.0, 2.0], &mut work, AT)
                .is_err()
        );
        drawing
            .pen
            .move_to(&mut work, Point::default(), AT)
            .unwrap();
        for invalid in [
            (Operator::Rlineto, &[1.0][..]),
            (Operator::Rrcurveto, &[1.0; 7]),
            (Operator::Rcurveline, &[1.0; 7]),
            (Operator::Rlinecurve, &[1.0; 9]),
            (Operator::Vvcurveto, &[1.0; 2]),
            (Operator::Hvcurveto, &[1.0; 6]),
        ] {
            assert!(
                drawing.draw(invalid.0, invalid.1, &mut work, AT).is_err(),
                "{invalid:?}"
            );
        }
        drawing
            .draw(Operator::Hlineto, &[10.0, 20.0, 30.0], &mut work, AT)
            .unwrap();
        assert_eq!(drawing.point, Point::new(40.0, 20.0));
        // hvcurveto: dx1, (dx2, dy2), then dy3 and the fifth operand dx3.
        drawing
            .draw(
                Operator::Hvcurveto,
                &[1.0, 2.0, 3.0, 4.0, 5.0],
                &mut work,
                AT,
            )
            .unwrap();
        assert_eq!(drawing.point, Point::new(48.0, 27.0));
    }

    #[test]
    fn flex_variants_expand_to_two_curves() {
        let hflex = flex(Escape::Hflex, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0], AT).unwrap();
        assert_eq!(
            hflex,
            [
                [1.0, 0.0, 2.0, 3.0, 4.0, 0.0],
                [5.0, 0.0, 6.0, -3.0, 7.0, 0.0]
            ]
        );
        // flex1 ends on the axis with the larger total displacement.
        let flex1 = flex(
            Escape::Flex1,
            &[10.0, 1.0, 10.0, 1.0, 10.0, 1.0, 10.0, 1.0, 10.0, 1.0, 9.0],
            AT,
        )
        .unwrap();
        assert_eq!(flex1[1][4..], [9.0, -5.0]);
        assert!(flex(Escape::Flex, &[1.0; 12], AT).is_err());
    }
}
