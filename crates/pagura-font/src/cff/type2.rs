//! Bounded, unhinted Type 2 charstrings (Adobe Technical Note 5177).
use super::{Cff, Private, dict, strings};
use crate::GlyphId;
use crate::read::{bad, cap, here, slice};
use crate::{recovery::repair, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostics, Limits, Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

struct Work<'a> {
    limits: &'a Limits,
    diagnostics: &'a mut Diagnostics,
    stopped: bool,
    operations: usize,
    segments: usize,
}
impl Work<'_> {
    fn operation(&mut self, at: usize) -> Result<()> {
        self.operations = self.operations.saturating_add(1);
        cap(self.operations, self.limits.operations, at)
    }
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
    transient: [f64; 32],
    path: PathBuilder,
    point: Point,
    open: bool,
    width: Option<f64>,
    hints: usize,
    seac: Option<[f64; 4]>,
    vsindex: usize,
    hint_phase: HintPhase,
    random: u32,
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

pub(super) fn glyph(
    font: &Cff,
    data: &[u8],
    glyph: GlyphId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<(Path, f64)> {
    let mut work = Work {
        limits,
        diagnostics,
        stopped: false,
        operations: 0,
        segments: 0,
    };
    load(font, data, glyph, 0, &mut work)
}
fn load(
    font: &Cff,
    data: &[u8],
    glyph: GlyphId,
    depth: usize,
    work: &mut Work<'_>,
) -> Result<(Path, f64)> {
    cap(depth, work.limits.depth.min(64), 0)?;
    let range = font
        .glyphs
        .get(u32_to_usize(glyph.0))
        .ok_or_else(|| bad(0, "glyph index outside CFF"))?;
    let private = font
        .private(glyph)
        .ok_or_else(|| bad(range.start, "glyph without a CFF Private DICT"))?;
    let mut state = State {
        // Type 2 charstrings hold at most 48 operands (CFF2: 513).
        stack: Vec::with_capacity(48),
        transient: [0.0; 32],
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
    };
    run(
        font,
        private,
        data,
        slice(data, range.clone(), "CFF charstring outside font")?,
        0,
        &mut state,
        work,
    )?;
    state.close(work, range.start)?;
    if let Some([dx, dy, base, accent]) = state.seac {
        if depth != 0 {
            return Err(bad(range.start, "nested seac composite"));
        }
        if font.cids.is_some() {
            return Err(bad(range.start, "seac in CID-keyed CFF"));
        }
        let mut component = |code: f64, transform: Transform| -> Result<()> {
            let code = u8::try_from(dict::integer(code, range.start)?)
                .map_err(|_| bad(range.start, "invalid seac character code"))?;
            let name = strings::standard(code)
                .ok_or_else(|| bad(range.start, "undefined seac character code"))?;
            let glyph = font
                .name(name.as_bytes())
                .ok_or_else(|| bad(range.start, "missing seac component"))?;
            let (path, _) = load(font, data, glyph, depth + 1, work)?;
            append(&mut state.path, &path, transform, work, range.start)
        };
        component(base, Transform::IDENTITY)?;
        component(accent, Transform::translate(dx, dy))?;
    }
    let path = state.path.finish().transformed(private.matrix);
    let width = state.width.unwrap_or(private.default_width) * private.matrix.a;
    if !width.is_finite() {
        return Err(crate::read::limit(
            range.start,
            "nonfinite charstring width",
        ));
    }
    Ok((path, width))
}
fn append(
    builder: &mut PathBuilder,
    path: &Path,
    transform: Transform,
    work: &mut Work<'_>,
    at: usize,
) -> Result<()> {
    for &segment in path.segments() {
        work.segment(at)?;
        match segment {
            Segment::MoveTo(p) => {
                builder.move_to(transform.apply(p));
            }
            Segment::LineTo(p) => {
                builder
                    .line_to(transform.apply(p))
                    .ok_or_else(|| bad(at, "invalid seac path"))?;
            }
            Segment::CubicTo(a, b, c) => {
                builder
                    .cubic_to(transform.apply(a), transform.apply(b), transform.apply(c))
                    .ok_or_else(|| bad(at, "invalid seac path"))?;
            }
            Segment::Close => {
                builder.close();
            }
        }
    }
    Ok(())
}
impl State {
    fn pop(&mut self, at: usize) -> Result<f64> {
        self.stack
            .pop()
            .ok_or_else(|| bad(at, "charstring stack underflow"))
    }
    fn count(&self, n: usize, at: usize) -> Result<()> {
        if self.stack.len() == n {
            Ok(())
        } else {
            Err(bad(at, "invalid charstring operand count"))
        }
    }
    fn width(&mut self, private: &Private, present: bool, at: usize) -> Result<()> {
        if self.width.is_none() {
            self.width = Some(if present {
                if self.stack.is_empty() {
                    return Err(bad(at, "missing charstring width"));
                }
                self.stack.remove(0) + private.nominal_width
            } else {
                private.default_width
            });
        }
        Ok(())
    }
    fn close(&mut self, work: &mut Work<'_>, at: usize) -> Result<()> {
        if self.open {
            work.segment(at)?;
            self.path.close();
            self.open = false;
        }
        Ok(())
    }
    fn point(&mut self, dx: f64, dy: f64, at: usize) -> Result<Point> {
        self.point.x += dx;
        self.point.y += dy;
        if !self.point.is_finite() {
            return Err(crate::read::limit(at, "nonfinite charstring coordinate"));
        }
        Ok(self.point)
    }
    fn moveto(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: usize) -> Result<()> {
        self.close(work, at)?;
        let p = self.point(dx, dy, at)?;
        work.segment(at)?;
        self.path.move_to(p);
        self.open = true;
        Ok(())
    }
    fn line(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: usize) -> Result<()> {
        if !self.open {
            return Err(bad(at, "charstring line before moveto"));
        }
        let p = self.point(dx, dy, at)?;
        work.segment(at)?;
        self.path
            .line_to(p)
            .ok_or_else(|| bad(at, "charstring line without current point"))?;
        Ok(())
    }
    fn curve(&mut self, deltas: &[f64], work: &mut Work<'_>, at: usize) -> Result<()> {
        if !self.open || deltas.len() != 6 {
            return Err(bad(at, "invalid charstring curve"));
        }
        let a = self.point(deltas[0], deltas[1], at)?;
        let b = self.point(deltas[2], deltas[3], at)?;
        let c = self.point(deltas[4], deltas[5], at)?;
        work.segment(at)?;
        self.path
            .cubic_to(a, b, c)
            .ok_or_else(|| bad(at, "charstring curve without current point"))?;
        Ok(())
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
    cap(
        depth,
        work.limits.depth.min(if font.cff2 { 64 } else { 10 }),
        here(&r),
    )?;
    if !font.cff2 {
        cap(r.remaining(), 65535, here(&r))?;
    }
    while !r.at_end() {
        let at = here(&r);
        work.operation(at)?;
        let byte = r.u8()?;
        if byte == 28 || byte >= 32 {
            cap(
                state.stack.len() + 1,
                if font.cff2 { 513 } else { 48 }.min(work.limits.value_nodes),
                at,
            )?;
            state.stack.push(dict::number(&mut r, byte, false)?);
            continue;
        }
        match byte {
            1 | 3 | 18 | 23 | 19 | 20 => {
                hints(font.cff2, private, byte, &mut r, state, at)?;
            }
            4 | 21 | 22 => {
                move_operator(font.cff2, private, byte, state, work, at)?;
            }
            5..=8 | 24..=27 | 30 | 31 => draw(byte, state, work, at)?,
            10 | 29 => {
                let subrs = if byte == 10 {
                    &private.subrs
                } else {
                    &font.global
                };
                let n = state.pop(at)?;
                let bias = if subrs.len() < 1240 {
                    107.0
                } else if subrs.len() < 33900 {
                    1131.0
                } else {
                    32768.0
                };
                let index = dict::integer(n + bias, at)?;
                let range = subrs
                    .get(index)
                    .ok_or_else(|| bad(at, "charstring subroutine index out of range"))?;
                if run(
                    font,
                    private,
                    data,
                    slice(data, range.clone(), "CFF subroutine outside font")?,
                    depth + 1,
                    state,
                    work,
                )? == Flow::End
                {
                    if !work.stopped && !r.at_end() {
                        return Err(bad(here(&r), "data after charstring end"));
                    }
                    return Ok(Flow::End);
                }
            }
            11 if !font.cff2 => {
                if depth == 0 {
                    return Err(bad(at, "return outside charstring subroutine"));
                }
                return Ok(Flow::Return);
            }
            12 => {
                if escape(r.u8()?, state, work, at)? {
                    return Ok(Flow::End);
                }
            }
            14 if !font.cff2 => {
                return finish(private, state, &r, at);
            }
            15 if font.cff2 => {
                state.count(1, at)?;
                state.vsindex = dict::integer(state.pop(at)?, at)?;
                if state.vsindex >= font.variations.len() {
                    return Err(bad(at, "CFF2 vsindex out of range"));
                }
            }
            16 if font.cff2 => blend(font, state, at)?,
            _ => return Err(bad(at, "invalid Type 2 charstring operator")),
        }
        cap(
            state.stack.len(),
            if font.cff2 { 513 } else { 48 }.min(work.limits.value_nodes),
            at,
        )?;
        // Finite operands are an invariant: arithmetic writes only the top
        // operand, and blend validates every value it writes in place.
        if state.stack.last().is_some_and(|n| !n.is_finite()) {
            return Err(crate::read::limit(at, "nonfinite charstring operand"));
        }
    }
    end_of_program(font.cff2, depth, state, here(&r))
}

fn end_of_program(cff2: bool, depth: usize, state: &State, at: usize) -> Result<Flow> {
    if cff2 {
        if depth == 0 && !state.stack.is_empty() {
            return Err(bad(at, "unconsumed CFF2 operands"));
        }
        Ok(if depth == 0 { Flow::End } else { Flow::Return })
    } else {
        Err(bad(at, "unterminated charstring"))
    }
}

fn blend(font: &Cff, state: &mut State, at: usize) -> Result<()> {
    let count = dict::integer(state.pop(at)?, at)?;
    let coefficients = font
        .variations
        .get(state.vsindex)
        .ok_or_else(|| bad(at, "missing CFF2 variation data"))?;
    let size = count
        .checked_mul(coefficients.len() + 1)
        .ok_or_else(|| crate::read::limit(at, "CFF2 blend count overflow"))?;
    if count == 0 || size > state.stack.len() {
        return Err(bad(at, "invalid CFF2 blend operands"));
    }
    let start = state.stack.len() - size;
    for i in 0..count {
        let mut value = state.stack[start + i];
        for (j, coefficient) in coefficients.iter().enumerate() {
            value += coefficient * state.stack[start + count + i * coefficients.len() + j];
        }
        if !value.is_finite() {
            return Err(crate::read::limit(at, "nonfinite charstring blend result"));
        }
        state.stack[start + i] = value;
    }
    state.stack.truncate(start + count);
    Ok(())
}

/// Takes the operands off the stack for one drawing operator; the stack's
/// storage returns, empty, for the next operands.
fn draw(op: u8, s: &mut State, w: &mut Work<'_>, at: usize) -> Result<()> {
    let mut args = std::mem::take(&mut s.stack);
    let result = draw_operands(op, &args, s, w, at);
    args.clear();
    s.stack = args;
    result
}

fn draw_operands(op: u8, args: &[f64], s: &mut State, w: &mut Work<'_>, at: usize) -> Result<()> {
    let n = args.len();
    match op {
        5 if n >= 2 && n.is_multiple_of(2) => {
            for a in args.as_chunks::<2>().0 {
                s.line(a[0], a[1], w, at)?;
            }
        }
        6 | 7 if n > 0 => {
            let mut horizontal = op == 6;
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
        8 if n >= 6 && n.is_multiple_of(6) => {
            for a in args.as_chunks::<6>().0 {
                s.curve(a, w, at)?;
            }
        }
        24 if n >= 8 && (n - 2).is_multiple_of(6) => {
            for a in args[..n - 2].as_chunks::<6>().0 {
                s.curve(a, w, at)?;
            }
            s.line(args[n - 2], args[n - 1], w, at)?;
        }
        25 if n >= 8 && (n - 6).is_multiple_of(2) => {
            for a in args[..n - 6].as_chunks::<2>().0 {
                s.line(a[0], a[1], w, at)?;
            }
            s.curve(&args[n - 6..], w, at)?;
        }
        26 | 27 if n >= 4 && matches!(n % 4, 0 | 1) => {
            let mut offset = n % 4;
            let mut extra = if offset == 1 { args[0] } else { 0.0 };
            while offset < n {
                let a = &args[offset..offset + 4];
                let deltas = if op == 26 {
                    [extra, a[0], a[1], a[2], 0.0, a[3]]
                } else {
                    [a[0], extra, a[1], a[2], a[3], 0.0]
                };
                s.curve(&deltas, w, at)?;
                extra = 0.0;
                offset += 4;
            }
        }
        30 | 31 if n >= 4 && matches!(n % 4, 0 | 1) => {
            let mut offset = 0;
            let mut horizontal = op == 31;
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
        _ => return Err(bad(at, "invalid charstring drawing operands")),
    }
    Ok(())
}

fn escape(op: u8, s: &mut State, w: &mut Work<'_>, at: usize) -> Result<bool> {
    match op {
        0 => {} // TN 5177 Appendix C: deprecated dotsection is a no-op.
        3 | 4 | 10 | 11 | 12 | 15 | 24 => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            let value = match op {
                3 => f64::from(a != 0.0 && b != 0.0),
                4 => f64::from(a != 0.0 || b != 0.0),
                10 => a + b,
                11 => a - b,
                12 => a / b,
                15 => f64::from(a == b),
                _ => a * b,
            };
            s.stack.push(value);
        }
        5 | 9 | 14 | 26 => {
            let a = s.pop(at)?;
            s.stack.push(match op {
                5 => f64::from(a == 0.0),
                9 => a.abs(),
                14 => -a,
                _ => a.sqrt(),
            });
        }
        18 => {
            s.pop(at)?;
        }
        20 => {
            let index = dict::integer(s.pop(at)?, at)?;
            let value = s.pop(at)?;
            *s.transient
                .get_mut(index)
                .ok_or_else(|| bad(at, "invalid transient array index"))? = value;
        }
        21 => {
            let index = dict::integer(s.pop(at)?, at)?;
            s.stack.push(
                *s.transient
                    .get(index)
                    .ok_or_else(|| bad(at, "invalid transient array index"))?,
            );
        }
        22 => {
            let v2 = s.pop(at)?;
            let v1 = s.pop(at)?;
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.push(if v1 <= v2 { a } else { b });
        }
        23 => {
            // TN 5177 4.4 specifies (0,1]; the algorithm is implementation policy.
            s.random = s.random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s.stack.push((f64::from(s.random) + 1.0) / 4_294_967_296.0);
        }
        27 => {
            let a = s.pop(at)?;
            s.stack.extend([a, a]);
        }
        28 => {
            let b = s.pop(at)?;
            let a = s.pop(at)?;
            s.stack.extend([b, a]);
        }
        29 => index(s, at)?,
        30 => {
            let shift = s.pop(at)?;
            let count = dict::integer(s.pop(at)?, at)?;
            if count > s.stack.len() || shift.fract() != 0.0 || shift.abs() > f64::from(i32::MAX) {
                return Err(bad(at, "invalid charstring roll"));
            }
            if count == 0 {
                return Ok(false);
            }
            let rotation = dict::integer(shift.rem_euclid(count as f64), at)?;
            let start = s.stack.len() - count;
            s.stack[start..].rotate_right(rotation);
        }
        34..=37 => flex(op, s, w, at)?,
        _ => {
            repair(
                w.diagnostics,
                repairs::CHARSTRING_OPERATOR,
                "unknown escaped Type 2 operator ends glyph",
                Position::Buffer(at),
            )?;
            s.stack.clear();
            w.stopped = true;
            return Ok(true);
        }
    }
    Ok(false)
}
fn flex(op: u8, s: &mut State, w: &mut Work<'_>, at: usize) -> Result<()> {
    let mut a = std::mem::take(&mut s.stack);
    let curves = flex_curves(op, &a, at);
    a.clear();
    s.stack = a;
    let (first, second) = curves?;
    s.curve(&first, w, at)?;
    s.curve(&second, w, at)
}

/// The two curves of a flex operator's operands.
fn flex_curves(op: u8, a: &[f64], at: usize) -> Result<([f64; 6], [f64; 6])> {
    Ok(match (op, a) {
        (34, [a, b, c, d, e, f, g]) => ([*a, 0.0, *b, *c, *d, 0.0], [*e, 0.0, *f, -c, *g, 0.0]),
        (35, [a, b, c, d, e, f, g, h, i, j, k, l, _]) => {
            ([*a, *b, *c, *d, *e, *f], [*g, *h, *i, *j, *k, *l])
        }
        (36, [a, b, c, d, e, f, g, h, i]) => (
            [*a, *b, *c, *d, *e, 0.0],
            [*f, 0.0, *g, *h, *i, -(b + d + h)],
        ),
        (37, [a, b, c, d, e, f, g, h, i, j, k]) => {
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
        _ => return Err(bad(at, "invalid flex operands")),
    })
}

fn hints(
    cff2: bool,
    private: &Private,
    byte: u8,
    r: &mut Cursor<'_>,
    state: &mut State,
    at: usize,
) -> Result<()> {
    if cff2 {
        state.width = Some(0.0);
    }
    state.width(private, state.stack.len() % 2 == 1, at)?;
    if !state.stack.len().is_multiple_of(2) {
        return Err(bad(at, "odd charstring hint operands"));
    }
    let mask = matches!(byte, 19 | 20);
    let phase = match byte {
        1 => HintPhase::Horizontal,
        18 => HintPhase::HorizontalMask,
        3 => HintPhase::Vertical,
        23 => HintPhase::VerticalMask,
        20 => HintPhase::Counter,
        _ => HintPhase::Mask,
    };
    if !cff2 {
        if (!mask || byte == 20) && phase < state.hint_phase
            || state.hint_phase >= HintPhase::Counter && !state.stack.is_empty()
        {
            return Err(bad(at, "charstring hints out of order"));
        }
        state.hint_phase = state.hint_phase.max(phase);
    }
    if !mask && state.stack.is_empty() {
        return Err(bad(at, "charstring stem operator without operands"));
    }
    state.hints += state.stack.len() / 2;
    cap(state.hints, 96, at)?;
    state.stack.clear();
    if mask {
        if state.hints == 0 {
            return Err(bad(at, "charstring mask without stems"));
        }
        let bytes = r.take(state.hints.div_ceil(8))?;
        let unused = (8 - state.hints % 8) % 8;
        if bytes
            .last()
            .is_some_and(|last| last & ((1 << unused) - 1) != 0)
        {
            return Err(bad(here(r) - 1, "nonzero charstring mask padding"));
        }
    }
    Ok(())
}

fn move_operator(
    cff2: bool,
    private: &Private,
    byte: u8,
    state: &mut State,
    work: &mut Work<'_>,
    at: usize,
) -> Result<()> {
    let n = if byte == 21 { 2 } else { 1 };
    if cff2 {
        state.width = Some(0.0);
    }
    state.width(private, state.stack.len() == n + 1, at)?;
    state.count(n, at)?;
    let (dx, dy) = match byte {
        4 => (0.0, state.stack[0]),
        22 => (state.stack[0], 0.0),
        _ => (state.stack[0], state.stack[1]),
    };
    state.stack.clear();
    state.hint_phase = HintPhase::Path;
    state.moveto(dx, dy, work, at)?;
    Ok(())
}

fn finish(private: &Private, state: &mut State, r: &Cursor<'_>, at: usize) -> Result<Flow> {
    state.width(private, matches!(state.stack.len(), 1 | 5), at)?;
    match state.stack.as_slice() {
        [] => {}
        [dx, dy, base, accent] => state.seac = Some([*dx, *dy, *base, *accent]),
        _ => return Err(bad(at, "invalid endchar operands")),
    }
    state.stack.clear();
    if !r.at_end() {
        return Err(bad(here(r), "data after charstring endchar"));
    }
    Ok(Flow::End)
}

fn index(s: &mut State, at: usize) -> Result<()> {
    let index = dict::integer(s.pop(at)?.max(0.0), at)?;
    if index >= s.stack.len() {
        return Err(bad(at, "charstring index outside stack"));
    }
    s.stack.push(s.stack[s.stack.len() - index - 1]);
    Ok(())
}
