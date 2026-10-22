//! Type 1 charstrings: unhinted outlines, subroutines, flex and
//! StandardEncoding seac (Type 1 chapters 6 and 8).
use super::Type1;
use crate::budget::Work;
use crate::cff::strings;
use crate::charstring::{Flow, Operands, integer, shared_number};
use crate::outline::{Pen, finite};
use crate::read::{syntax, unsupported};
use crate::{GlyphId, features, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use pagura_raster::{Path, Point, Transform};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Outline,
    Width,
}

/// One glyph's outline, advance width and side-bearing point.
struct Glyph {
    path: Path,
    width: f64,
    bearing: Point,
}

/// The operands of `seac` (Type 1 6.4).
#[derive(Debug, Clone, Copy)]
struct Seac {
    asb: f64,
    dx: f64,
    dy: f64,
    base: f64,
    accent: f64,
}

/// The points collected by a flex in progress (Type 1 8.3).
#[derive(Debug, Clone, Copy, Default)]
struct Flex {
    points: [Point; FLEX_POINTS],
    len: usize,
}

struct State {
    operands: Operands<MAX_OPERANDS>,
    results: Operands<MAX_OPERANDS>,
    pen: Pen,
    point: Point,
    width: Option<f64>,
    bearing: Point,
    seac: Option<Seac>,
    flex: Option<Flex>,
}

struct Run<'r, 'w> {
    font: &'r Type1,
    work: &'r mut Work<'w>,
    mode: Mode,
    /// A repair or a width request ended the glyph before its end.
    stopped: bool,
}

pub(super) fn outline(font: &Type1, glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
    run(font, glyph, Mode::Outline, work).map(|g| g.path)
}

pub(super) fn advance(font: &Type1, glyph: GlyphId, work: &mut Work<'_>) -> Result<f64> {
    run(font, glyph, Mode::Width, work).map(|g| g.width)
}

fn run(font: &Type1, glyph: GlyphId, mode: Mode, work: &mut Work<'_>) -> Result<Glyph> {
    Run {
        font,
        work,
        mode,
        stopped: false,
    }
    .load(glyph, 0)
}

impl Run<'_, '_> {
    fn load(&mut self, glyph: GlyphId, depth: usize) -> Result<Glyph> {
        let charstring = self
            .font
            .glyphs
            .get(u32_to_usize(glyph.0))
            .ok_or_else(|| syntax(Position::Unknown, "glyph index outside Type 1 font"))?;
        let r = charstring.cursor(&self.font.plain)?;
        let (start, end) = (r.position(), Position::Buffer(charstring.end()));
        self.work.depth(depth, usize::MAX, start)?;
        let mut s = State {
            operands: Operands::new(),
            results: Operands::new(),
            pen: Pen::new(),
            point: Point::default(),
            width: None,
            bearing: Point::default(),
            seac: None,
            flex: None,
        };
        self.charstring(r, 0, &mut s)?;
        if s.flex.is_some() {
            return Err(syntax(start, "unterminated Type 1 flex"));
        }
        let width = s
            .width
            .ok_or_else(|| syntax(start, "Type 1 charstring has no width"))?;
        if self.mode == Mode::Width {
            return Ok(Glyph {
                path: Path::default(),
                width,
                bearing: s.bearing,
            });
        }
        s.pen.close(self.work, end)?;
        if let Some(seac) = s.seac {
            if depth != 0 {
                return Err(syntax(start, "nested Type 1 seac composite"));
            }
            for (code, transform, base) in [
                (seac.base, Transform::IDENTITY, true),
                (seac.accent, Transform::translate(seac.dx, seac.dy), false),
            ] {
                let code = u8::try_from(integer(code, start)?)
                    .map_err(|_| syntax(start, "invalid Type 1 seac code"))?;
                let name = strings::standard(code)
                    .ok_or_else(|| syntax(start, "undefined Type 1 seac code"))?;
                let component = self
                    .font
                    .name(name.as_bytes())
                    .ok_or_else(|| syntax(start, "missing Type 1 seac component"))?;
                let component = self.load(component, depth + 1)?;
                // Components already include their side bearings, so the
                // accent origin moves by (adx, ady) only (Type 1 3.1, 6.4).
                let consistent = if base {
                    component.width == width && component.bearing == s.bearing
                } else {
                    component.bearing.x == seac.asb
                };
                if !consistent {
                    return Err(syntax(start, "inconsistent Type 1 seac metrics"));
                }
                s.pen.append(self.work, &component.path, transform, start)?;
            }
        }
        Ok(Glyph {
            path: s.pen.finish(Transform::IDENTITY, start)?,
            width,
            bearing: s.bearing,
        })
    }

    fn charstring(&mut self, mut r: Cursor<'_>, depth: usize, s: &mut State) -> Result<Flow> {
        self.work.depth(depth, MAX_SUBR_DEPTH, r.position())?;
        while !r.at_end() {
            let at = r.position();
            self.work.work(1, at)?;
            let byte = r.u8()?;
            if byte >= 32 {
                let n = match shared_number(byte, &mut r)? {
                    Some(n) => n,
                    None if byte == LONG_INT => f64::from(r.i32()?),
                    None => return Err(syntax(at, "invalid Type 1 number")),
                };
                s.operands.push(n, at)?;
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
                    s.operands.exactly(2, at)?;
                    s.operands.clear();
                }
                Operator::Vmoveto | Operator::Rmoveto | Operator::Hmoveto => {
                    let (dx, dy) = match (op, s.operands.as_slice()) {
                        (Operator::Vmoveto, &[dy]) => (0.0, dy),
                        (Operator::Hmoveto, &[dx]) => (dx, 0.0),
                        (Operator::Rmoveto, &[dx, dy]) => (dx, dy),
                        _ => return Err(syntax(at, "invalid Type 1 operand count")),
                    };
                    s.operands.clear();
                    s.move_to(dx, dy, self.work, at)?;
                }
                Operator::Closepath => {
                    s.operands.exactly(0, at)?;
                    s.pen.close(self.work, at)?;
                }
                Operator::Callsubr => {
                    let index = integer(s.operands.pop(at)?, at)?;
                    let subr = self
                        .font
                        .subrs
                        .get(index)
                        .ok_or_else(|| syntax(at, "Type 1 subroutine index out of range"))?;
                    if self.charstring(subr.cursor(&self.font.plain)?, depth + 1, s)? == Flow::End {
                        return self.end(&r, "data after Type 1 character end");
                    }
                }
                Operator::Return => {
                    if depth == 0 {
                        return Err(syntax(at, "Type 1 return outside subroutine"));
                    }
                    return Ok(Flow::Return);
                }
                Operator::Escape => {
                    let escape = Escape::try_from(r.u8()?);
                    if s.width.is_none() && !matches!(escape, Ok(Escape::Sbw | Escape::Div)) {
                        return Err(syntax(at, "Type 1 width must be the first command"));
                    }
                    let Ok(escape) = escape else {
                        self.work.repair(
                            repairs::CHARSTRING_OPERATOR,
                            "unknown escaped Type 1 operator ends glyph",
                            self.font.input(at),
                        )?;
                        s.operands.clear();
                        self.stopped = true;
                        return Ok(Flow::End);
                    };
                    if self.escape(escape, s, at)? {
                        return self.end(&r, "data after Type 1 seac");
                    }
                }
                Operator::Hsbw => {
                    let &[sbx, wx] = s.operands.exactly(2, at)? else {
                        return Err(syntax(at, "invalid Type 1 operand count"));
                    };
                    s.set_width(Point::new(sbx, 0.0), wx, at)?;
                }
                Operator::Endchar => {
                    s.operands.exactly(0, at)?;
                    return self.end(&r, "data after Type 1 endchar");
                }
                _ => {
                    s.draw(op, self.work, at)?;
                    s.operands.clear();
                }
            }
            if self.mode == Mode::Width && s.width.is_some() {
                self.stopped = true;
                return Ok(Flow::End);
            }
        }
        Err(r.error("unterminated Type 1 charstring"))
    }

    /// The end of the glyph: nothing may follow unless a repair stopped it.
    fn end(&self, r: &Cursor<'_>, message: &'static str) -> Result<Flow> {
        if !self.stopped && !r.at_end() {
            return Err(r.error(message));
        }
        Ok(Flow::End)
    }

    /// An escaped command; `true` if it ends the glyph (`seac`).
    fn escape(&mut self, op: Escape, s: &mut State, at: Position) -> Result<bool> {
        match op {
            Escape::Dotsection => {
                s.operands.exactly(0, at)?; // Dotsection affects hints only.
            }
            Escape::Vstem3 | Escape::Hstem3 => {
                s.operands.exactly(6, at)?;
                s.operands.clear();
            }
            Escape::Seac => {
                let &[asb, dx, dy, base, accent] = s.operands.exactly(5, at)? else {
                    return Err(syntax(at, "invalid Type 1 operand count"));
                };
                s.seac = Some(Seac {
                    asb,
                    dx,
                    dy,
                    base,
                    accent,
                });
                s.operands.clear();
                return Ok(true);
            }
            Escape::Sbw => {
                let &[sbx, sby, wx, wy] = s.operands.exactly(4, at)? else {
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
                let b = s.operands.pop(at)?;
                let a = s.operands.pop(at)?;
                s.operands.push(a / b, at)?;
            }
            Escape::Callothersubr => s.othersubr(self.work, at)?,
            Escape::Pop => {
                let value = s.results.pop(at)?;
                s.operands.push(value, at)?;
            }
            Escape::Setcurrentpoint => {
                let &[x, y] = s.operands.exactly(2, at)? else {
                    return Err(syntax(at, "invalid Type 1 operand count"));
                };
                s.point = Point::new(x, y);
                s.operands.clear();
            }
        }
        Ok(false)
    }
}

impl State {
    fn set_width(&mut self, bearing: Point, width: f64, at: Position) -> Result<()> {
        if self.width.is_some() {
            return Err(syntax(at, "duplicate Type 1 width"));
        }
        self.point = bearing;
        self.bearing = bearing;
        self.width = Some(width);
        self.operands.clear();
        Ok(())
    }

    fn relative(&mut self, dx: f64, dy: f64, at: Position) -> Result<Point> {
        self.point = Point::new(self.point.x + dx, self.point.y + dy);
        finite(self.point, at)?;
        Ok(self.point)
    }

    fn move_to(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: Position) -> Result<()> {
        let to = self.relative(dx, dy, at)?;
        // During flex, moves only collect points (Type 1 8.3).
        if self.flex.is_none() {
            self.pen.move_to(work, to, at)?;
        }
        Ok(())
    }

    // Type 1 6.4 keeps the charstring current point across closepath.
    // PathBuilder uses PDF close semantics, so a resumed contour must
    // explicitly begin at the retained Type 1 point.
    fn resume(&mut self, work: &mut Work<'_>, at: Position) -> Result<()> {
        if self.flex.is_some() || self.pen.current().is_none() {
            return Err(syntax(at, "Type 1 drawing without a current path"));
        }
        if !self.pen.is_open() {
            self.pen.move_to(work, self.point, at)?;
        }
        Ok(())
    }

    fn line(&mut self, dx: f64, dy: f64, work: &mut Work<'_>, at: Position) -> Result<()> {
        self.resume(work, at)?;
        let to = self.relative(dx, dy, at)?;
        self.pen.line_to(work, to, at)
    }

    fn curve(
        &mut self,
        [dxa, dya, dxb, dyb, dxc, dyc]: [f64; 6],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<()> {
        self.resume(work, at)?;
        let points = [
            self.relative(dxa, dya, at)?,
            self.relative(dxb, dyb, at)?,
            self.relative(dxc, dyc, at)?,
        ];
        self.pen.cubic_to(work, points, at)
    }

    fn draw(&mut self, op: Operator, work: &mut Work<'_>, at: Position) -> Result<()> {
        let curve = match (op, self.operands.as_slice()) {
            (Operator::Rlineto, &[dx, dy]) => return self.line(dx, dy, work, at),
            (Operator::Hlineto, &[dx]) => return self.line(dx, 0.0, work, at),
            (Operator::Vlineto, &[dy]) => return self.line(0.0, dy, work, at),
            (Operator::Rrcurveto, &[a, b, c, d, e, f]) => [a, b, c, d, e, f],
            (Operator::Vhcurveto, &[a, b, c, d]) => [0.0, a, b, c, d, 0.0],
            (Operator::Hvcurveto, &[a, b, c, d]) => [a, 0.0, b, c, 0.0, d],
            _ => return Err(syntax(at, "invalid Type 1 drawing operands")),
        };
        self.curve(curve, work, at)
    }

    /// `callothersubr`: the standard flex and hint-replacement OtherSubrs,
    /// and chapter 8's argument fallback for the others.
    fn othersubr(&mut self, work: &mut Work<'_>, at: Position) -> Result<()> {
        let subr = integer(self.operands.pop(at)?, at)?;
        let count = integer(self.operands.pop(at)?, at)?;
        let split = self
            .operands
            .len()
            .checked_sub(count)
            .ok_or_else(|| syntax(at, "Type 1 othersubr stack underflow"))?;
        let args = self
            .operands
            .as_slice()
            .get(split..)
            .unwrap_or_default()
            .to_vec();
        self.operands.truncate(split);
        match (subr, args.as_slice()) {
            (FLEX_START, []) => {
                if self.flex.is_some() {
                    return Err(syntax(at, "nested Type 1 flex"));
                }
                self.flex = Some(Flex::default());
            }
            (FLEX_POINT, []) => {
                let flex = self
                    .flex
                    .as_mut()
                    .ok_or_else(|| syntax(at, "Type 1 flex point without flex start"))?;
                *flex
                    .points
                    .get_mut(flex.len)
                    .ok_or_else(|| syntax(at, "Type 1 flex has more than seven points"))? =
                    self.point;
                flex.len += 1;
            }
            (FLEX_END, &[_, x, y]) => {
                let flex = self
                    .flex
                    .take()
                    .ok_or_else(|| syntax(at, "Type 1 flex end without start"))?;
                let [_reference, a, b, c, d, e, f] = flex.points;
                if flex.len != FLEX_POINTS {
                    return Err(syntax(at, "Type 1 flex needs seven points"));
                }
                self.pen.cubic_to(work, [a, b, c], at)?;
                self.pen.cubic_to(work, [d, e, f], at)?;
                self.results.push(y, at)?;
                self.results.push(x, at)?;
            }
            // Hint replacement returns the subroutine number.
            (HINT_REPLACEMENT, &[number]) => self.results.push(number, at)?,
            (FLEX_END..=HINT_REPLACEMENT, _) => {
                return Err(syntax(at, "invalid standard Type 1 OtherSubr arguments"));
            }
            // Type 1 chapter 8 fallback: the arguments return in order,
            // arg1 popped first.
            _ => {
                for &value in args.iter().rev() {
                    self.results.push(value, at)?;
                }
            }
        }
        Ok(())
    }
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
            plain: charstring.to_vec(),
            glyphs: vec![Charstring::new(0, charstring.len(), None)],
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
    fn charstrings_are_decrypted_when_first_drawn() {
        // A plain glyph and an encrypted one too short for its lenIV: the
        // damage surfaces only when that glyph is drawn, at its plaintext
        // offset, and the decryption of a good glyph is kept.
        let mut program = HSBW.to_vec();
        program.extend([239, 239, 21, 14]);
        let mut font = font(&program);
        let good = program.len();
        font.plain.extend([1, 2]);
        font.glyphs.push(Charstring::new(good, 2, Some(4)));
        font.glyph_count = 2;
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        assert!(outline(&font, GlyphId(0), &mut work).is_ok());
        let error = outline(&font, GlyphId(1), &mut work).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, pagura_core::Position::Buffer(good))
        );
        let encrypted = Charstring::new(0, 6, Some(4));
        let cipher = [1, 2, 3, 4, 5, 6];
        let bytes = encrypted.bytes(&cipher).unwrap().to_vec();
        assert_eq!(bytes.len(), 2);
        assert!(std::ptr::eq(
            encrypted.bytes(&cipher).unwrap(),
            encrypted.decrypted.get().unwrap().as_slice()
        ));
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
