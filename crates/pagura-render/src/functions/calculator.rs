//! Capability-free PostScript calculator subset (7.10.5).
//! Braces compile into conditional branches, never procedure objects; only numeric/stack operators
//! execute. There is no file, network, process, name lookup or external exec.

use pagura_core::{Error, Limits, ObjectId, Position, Result};
use std::sync::Arc;

/// The operand stack depth Annex C (Table C.1) gives for PostScript
/// calculator functions; deeper stacks are refused.
const STACK: usize = 100;

#[derive(Debug, Clone)]
pub(super) struct Calculator {
    program: Arc<[Instruction]>,
    object: ObjectId,
}
#[derive(Debug, Clone)]
enum Instruction {
    Push(Operand),
    Op {
        op: PsOp,
        offset: usize,
    },
    /// `{then} if` or `{then} {otherwise} ifelse`.
    Conditional {
        then: Arc<[Instruction]>,
        otherwise: Option<Arc<[Instruction]>>,
        offset: usize,
    },
}
#[derive(Debug, Clone, Copy)]
enum Operand {
    Integer(i32),
    Real(f64),
    Boolean(bool),
}

/// The operators of Table 42, except the conditionals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PsOp {
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}
impl PsOp {
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "abs" => Self::Abs,
            "add" => Self::Add,
            "atan" => Self::Atan,
            "ceiling" => Self::Ceiling,
            "cos" => Self::Cos,
            "cvi" => Self::Cvi,
            "cvr" => Self::Cvr,
            "div" => Self::Div,
            "exp" => Self::Exp,
            "floor" => Self::Floor,
            "idiv" => Self::Idiv,
            "ln" => Self::Ln,
            "log" => Self::Log,
            "mod" => Self::Mod,
            "mul" => Self::Mul,
            "neg" => Self::Neg,
            "round" => Self::Round,
            "sin" => Self::Sin,
            "sqrt" => Self::Sqrt,
            "sub" => Self::Sub,
            "truncate" => Self::Truncate,
            "and" => Self::And,
            "bitshift" => Self::Bitshift,
            "eq" => Self::Eq,
            "ge" => Self::Ge,
            "gt" => Self::Gt,
            "le" => Self::Le,
            "lt" => Self::Lt,
            "ne" => Self::Ne,
            "not" => Self::Not,
            "or" => Self::Or,
            "xor" => Self::Xor,
            "copy" => Self::Copy,
            "dup" => Self::Dup,
            "exch" => Self::Exch,
            "index" => Self::Index,
            "pop" => Self::Pop,
            "roll" => Self::Roll,
            _ => return None,
        })
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    nodes: usize,
    limits: &'a Limits,
}
impl<'a> Parser<'a> {
    fn whitespace(&mut self) {
        loop {
            while self
                .bytes
                .get(self.offset)
                .is_some_and(|b| pagura_core::syntax::is_whitespace(*b))
            {
                self.offset += 1;
            }
            if self.bytes.get(self.offset) != Some(&b'%') {
                break;
            }
            while self
                .bytes
                .get(self.offset)
                .is_some_and(|b| !matches!(b, b'\r' | b'\n'))
            {
                self.offset += 1;
            }
        }
    }
    fn procedure(&mut self, depth: usize) -> Result<Arc<[Instruction]>> {
        if depth >= self.limits.depth {
            return Err(Error::limit("calculator procedure nesting exceeded"));
        }
        self.whitespace();
        if self.bytes.get(self.offset) != Some(&b'{') {
            return Err(Error::syntax("calculator requires a procedure"));
        }
        self.offset += 1;
        let mut program = Vec::new();
        loop {
            self.whitespace();
            if self.bytes.get(self.offset) == Some(&b'}') {
                self.offset += 1;
                return Ok(program.into());
            }
            if self.nodes >= self.limits.value_nodes {
                return Err(Error::limit("calculator instruction count exceeded"));
            }
            self.nodes += 1;
            let instruction = if self.bytes.get(self.offset) == Some(&b'{') {
                let offset = self.offset;
                let then = self.procedure(depth + 1)?;
                self.whitespace();
                let otherwise = if self.bytes.get(self.offset) == Some(&b'{') {
                    Some(self.procedure(depth + 1)?)
                } else {
                    None
                };
                let (word, _) = self.word()?;
                if word != if otherwise.is_some() { "ifelse" } else { "if" } {
                    return Err(Error::syntax(
                        "calculator brace expression requires conditional operator",
                    ));
                }
                Instruction::Conditional {
                    then,
                    otherwise,
                    offset,
                }
            } else {
                let (word, start) = self.word()?;
                match word {
                    "true" => Instruction::Push(Operand::Boolean(true)),
                    "false" => Instruction::Push(Operand::Boolean(false)),
                    "if" | "ifelse" => {
                        return Err(Error::syntax(
                            "calculator conditional requires brace expressions",
                        ));
                    }
                    _ => {
                        if let Some(op) = PsOp::parse(word) {
                            Instruction::Op { op, offset: start }
                        } else if let Ok(n) = word.parse::<i32>() {
                            Instruction::Push(Operand::Integer(n))
                        } else {
                            Instruction::Push(Operand::Real(self.real(word)?))
                        }
                    }
                }
            };
            program.push(instruction);
        }
    }
    /// A real in PDF syntax (7.3.3): no exponent and no radix.
    fn real(&self, word: &str) -> Result<f64> {
        let mut parser = pagura_core::syntax::Parser::new(
            word.as_bytes(),
            pagura_core::syntax::Origin::Buffer,
            *self.limits,
        );
        parser
            .parse()
            .ok()
            .filter(|_| parser.at_end())
            .and_then(|v| v.as_number())
            .ok_or(Error::syntax(
                "unknown calculator operator or invalid PDF number",
            ))
    }
    fn word(&mut self) -> Result<(&'a str, usize)> {
        self.whitespace();
        let start = self.offset;
        while self.bytes.get(self.offset).is_some_and(|b| {
            !pagura_core::syntax::is_whitespace(*b) && !matches!(b, b'{' | b'}' | b'%')
        }) {
            self.offset += 1;
        }
        if start == self.offset {
            return Err(Error::syntax("unterminated calculator expression"));
        }
        Ok((
            std::str::from_utf8(
                self.bytes
                    .get(start..self.offset)
                    .ok_or(Error::syntax("invalid calculator token"))?,
            )
            .map_err(|_| Error::syntax("invalid calculator token"))?,
            start,
        ))
    }
}
impl Calculator {
    pub(super) fn parse(bytes: &[u8], limits: &Limits, object: ObjectId) -> Result<Self> {
        let mut parser = Parser {
            bytes,
            offset: 0,
            nodes: 0,
            limits,
        };
        let program = parser.procedure(0).map_err(|e| {
            e.at(Position::Stream {
                object,
                offset: parser.offset,
            })
        })?;
        parser.whitespace();
        if parser.offset != bytes.len() {
            return Err(Error::syntax("trailing calculator program data"));
        }
        Ok(Self { program, object })
    }
    /// Runs the program on `input` and writes the stack, which must hold
    /// exactly `output.len()` numbers, to `output`.
    pub(super) fn evaluate(
        &self,
        input: &[f64],
        output: &mut [f64],
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        let mut stack = Stack {
            values: [Operand::Integer(0); STACK],
            len: 0,
            limits,
            work,
            object: self.object,
        };
        for &value in input {
            stack.push(Operand::Real(value))?;
        }
        stack.execute(&self.program, 0)?;
        if stack.len != output.len() {
            return Err(Error::syntax("wrong calculator output count"));
        }
        for (out, value) in output.iter_mut().zip(&stack.values) {
            *out = numeric(*value)?;
        }
        Ok(())
    }
}
struct Stack<'a> {
    values: [Operand; STACK],
    len: usize,
    limits: &'a Limits,
    work: &'a mut usize,
    object: ObjectId,
}
impl Stack<'_> {
    fn charge(&mut self, count: usize) -> Result<()> {
        *self.work = self
            .work
            .checked_add(count)
            .filter(|&n| n <= self.limits.operations)
            .ok_or(Error::limit("calculator work exceeded"))?;
        Ok(())
    }
    fn push(&mut self, value: Operand) -> Result<()> {
        let slot = self
            .values
            .get_mut(self.len)
            .ok_or(Error::limit("calculator stack exceeded"))?;
        if let Operand::Real(v) = value
            && !v.is_finite()
        {
            return Err(Error::syntax(
                "calculator arithmetic is undefined or overflows",
            ));
        }
        *slot = value;
        self.len += 1;
        Ok(())
    }
    fn pop(&mut self) -> Result<Operand> {
        self.len = self
            .len
            .checked_sub(1)
            .ok_or(Error::syntax("calculator stack underflow"))?;
        Ok(self.values[self.len])
    }
    /// The operands from depth `count` to the top, oldest first.
    fn top(&self, count: usize, underflow: &'static str) -> Result<std::ops::Range<usize>> {
        let start = self
            .len
            .checked_sub(count)
            .ok_or(Error::syntax(underflow))?;
        Ok(start..self.len)
    }
    fn integer(&mut self) -> Result<i32> {
        if let Operand::Integer(v) = self.pop()? {
            Ok(v)
        } else {
            Err(Error::syntax("calculator requires integer"))
        }
    }
    fn execute(&mut self, program: &[Instruction], depth: usize) -> Result<()> {
        if depth >= self.limits.depth {
            return Err(Error::limit("calculator execution nesting exceeded"));
        }
        for instruction in program {
            self.charge(1)?;
            match instruction {
                Instruction::Push(value) => self.push(*value)?,
                Instruction::Conditional {
                    then,
                    otherwise,
                    offset,
                } => {
                    let Operand::Boolean(test) = self.pop()? else {
                        return Err(Error::syntax("calculator condition must be boolean").at(
                            Position::Stream {
                                object: self.object,
                                offset: *offset,
                            },
                        ));
                    };
                    if test {
                        self.execute(then, depth + 1)?;
                    } else if let Some(otherwise) = otherwise {
                        self.execute(otherwise, depth + 1)?;
                    }
                }
                Instruction::Op { op, offset } => self.operator(*op).map_err(|e| {
                    e.at(Position::Stream {
                        object: self.object,
                        offset: *offset,
                    })
                })?,
            }
        }
        Ok(())
    }
    fn operator(&mut self, op: PsOp) -> Result<()> {
        use PsOp as P;
        match op {
            P::Dup | P::Exch | P::Pop | P::Copy | P::Index | P::Roll => self.stack_operator(op),
            P::And | P::Or | P::Xor | P::Not | P::Bitshift => self.logical(op),
            P::Eq | P::Ne | P::Ge | P::Gt | P::Le | P::Lt => self.compare(op),
            P::Add | P::Sub | P::Mul | P::Div | P::Idiv | P::Mod | P::Exp | P::Atan => {
                self.binary(op)
            }
            P::Abs
            | P::Ceiling
            | P::Cos
            | P::Cvi
            | P::Cvr
            | P::Floor
            | P::Ln
            | P::Log
            | P::Neg
            | P::Round
            | P::Sin
            | P::Sqrt
            | P::Truncate => self.unary(op),
        }
    }
    fn stack_operator(&mut self, op: PsOp) -> Result<()> {
        match op {
            PsOp::Pop => {
                self.pop()?;
            }
            PsOp::Dup => {
                let value = self.pop()?;
                self.push(value)?;
                self.push(value)?;
            }
            PsOp::Exch => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.push(a)?;
                self.push(b)?;
            }
            PsOp::Copy => {
                let count = usize::try_from(self.integer()?)
                    .map_err(|_| Error::syntax("negative calculator copy"))?;
                let copied = self.top(count, "calculator copy underflow")?;
                if self.len.checked_add(count).is_none_or(|n| n > STACK) {
                    return Err(Error::limit("calculator stack exceeded"));
                }
                self.charge(count)?;
                self.values.copy_within(copied, self.len);
                self.len += count;
            }
            PsOp::Index => {
                let n = usize::try_from(self.integer()?)
                    .map_err(|_| Error::syntax("negative calculator index"))?;
                let below = self.top(n.saturating_add(1), "calculator index underflow")?;
                self.push(self.values[below.start])?;
            }
            PsOp::Roll => {
                let shift = self.integer()?;
                let n = self.integer()?;
                if n < 0 {
                    return Err(Error::syntax("negative calculator roll"));
                }
                let count =
                    usize::try_from(n).map_err(|_| Error::limit("roll size exceeds usize"))?;
                let rolled = self.top(count, "calculator roll underflow")?;
                self.charge(count)?;
                if n > 0 {
                    let shift = usize::try_from(shift.rem_euclid(n))
                        .map_err(|_| Error::limit("roll shift exceeds usize"))?;
                    self.values[rolled].rotate_right(shift);
                }
            }
            _ => return Err(Error::syntax("unknown calculator stack operator")),
        }
        Ok(())
    }
    fn binary(&mut self, op: PsOp) -> Result<()> {
        if matches!(op, PsOp::Idiv | PsOp::Mod) {
            let b = self.integer()?;
            let a = self.integer()?;
            if op == PsOp::Mod && a == i32::MIN && b == -1 {
                return self.push(Operand::Integer(0));
            }
            let result = if op == PsOp::Idiv {
                a.checked_div(b)
            } else {
                a.checked_rem(b)
            }
            .ok_or(Error::syntax("undefined calculator integer division"))?;
            return self.push(Operand::Integer(result));
        }
        let b = self.pop()?;
        let a = self.pop()?;
        if let (Operand::Integer(a), Operand::Integer(b)) = (a, b) {
            let result = match op {
                PsOp::Add => a.checked_add(b),
                PsOp::Sub => a.checked_sub(b),
                PsOp::Mul => a.checked_mul(b),
                _ => None,
            };
            if let Some(result) = result {
                return self.push(Operand::Integer(result));
            }
        }
        let (a, b) = (numeric(a)?, numeric(b)?);
        let result = match op {
            PsOp::Add => a + b,
            PsOp::Sub => a - b,
            PsOp::Mul => a * b,
            PsOp::Div => {
                if b == 0.0 {
                    return Err(Error::syntax("calculator division by zero"));
                }
                a / b
            }
            PsOp::Exp => a.powf(b),
            PsOp::Atan => {
                if a == 0.0 && b == 0.0 {
                    return Err(Error::syntax("undefined calculator atan"));
                }
                a.atan2(b).to_degrees().rem_euclid(360.0)
            }
            _ => return Err(Error::syntax("unknown calculator arithmetic operator")),
        };
        self.push(Operand::Real(result))
    }
    fn unary(&mut self, op: PsOp) -> Result<()> {
        let value = self.pop()?;
        if let Operand::Integer(v) = value {
            if matches!(
                op,
                PsOp::Floor | PsOp::Ceiling | PsOp::Round | PsOp::Truncate | PsOp::Cvi
            ) {
                return self.push(Operand::Integer(v));
            }
            if let Some(v) = match op {
                PsOp::Abs => v.checked_abs(),
                PsOp::Neg => v.checked_neg(),
                _ => None,
            } {
                return self.push(Operand::Integer(v));
            }
        }
        let v = numeric(value)?;
        if op == PsOp::Cvi {
            let v = v.trunc();
            if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&v) {
                return Err(Error::syntax("calculator integer conversion out of range"));
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "truncated real is bounded to i32 above"
            )]
            return self.push(Operand::Integer(v as i32));
        }
        let result = match op {
            PsOp::Abs => v.abs(),
            PsOp::Neg => -v,
            PsOp::Floor => v.floor(),
            PsOp::Ceiling => v.ceil(),
            PsOp::Round => (v + 0.5).floor(),
            PsOp::Truncate => v.trunc(),
            PsOp::Cvr => v,
            PsOp::Sqrt => v.sqrt(),
            PsOp::Ln | PsOp::Log => {
                if v <= 0.0 {
                    return Err(Error::syntax("calculator log outside domain"));
                }
                if op == PsOp::Ln { v.ln() } else { v.log10() }
            }
            PsOp::Sin => v.to_radians().sin(),
            PsOp::Cos => v.to_radians().cos(),
            _ => return Err(Error::syntax("unknown calculator unary operator")),
        };
        self.push(Operand::Real(result))
    }
    fn logical(&mut self, op: PsOp) -> Result<()> {
        if op == PsOp::Bitshift {
            let shift = self.integer()?;
            let value = self.integer()?.cast_unsigned();
            let amount = shift.unsigned_abs();
            let result = if amount >= 32 {
                0
            } else if shift >= 0 {
                value << amount
            } else {
                value >> amount
            };
            return self.push(Operand::Integer(result.cast_signed()));
        }
        let b = self.pop()?;
        if op == PsOp::Not {
            return self.push(match b {
                Operand::Integer(v) => Operand::Integer(!v),
                Operand::Boolean(v) => Operand::Boolean(!v),
                Operand::Real(_) => {
                    return Err(Error::syntax("invalid calculator logical operand"));
                }
            });
        }
        let a = self.pop()?;
        self.push(match (a, b) {
            (Operand::Integer(a), Operand::Integer(b)) => Operand::Integer(match op {
                PsOp::And => a & b,
                PsOp::Or => a | b,
                _ => a ^ b,
            }),
            (Operand::Boolean(a), Operand::Boolean(b)) => Operand::Boolean(match op {
                PsOp::And => a & b,
                PsOp::Or => a | b,
                _ => a ^ b,
            }),
            _ => return Err(Error::syntax("mismatched calculator logical operands")),
        })
    }
    fn compare(&mut self, op: PsOp) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let equal = match (a, b) {
            (Operand::Boolean(a), Operand::Boolean(b)) => a == b,
            _ => numeric(a)
                .ok()
                .zip(numeric(b).ok())
                .is_some_and(|(a, b)| a == b),
        };
        let result = match op {
            PsOp::Eq => equal,
            PsOp::Ne => !equal,
            _ => {
                let (a, b) = (numeric(a)?, numeric(b)?);
                match op {
                    PsOp::Ge => a >= b,
                    PsOp::Gt => a > b,
                    PsOp::Le => a <= b,
                    PsOp::Lt => a < b,
                    _ => return Err(Error::syntax("unknown calculator comparison")),
                }
            }
        };
        self.push(Operand::Boolean(result))
    }
}
fn numeric(value: Operand) -> Result<f64> {
    match value {
        Operand::Integer(n) => Ok(f64::from(n)),
        Operand::Real(n) => Ok(n),
        Operand::Boolean(_) => Err(Error::syntax("calculator requires numeric operand")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Calculator, STACK};
    use pagura_core::{ErrorKind, Limits, ObjectId};

    fn run(program: &str, limits: &Limits) -> pagura_core::Result<f64> {
        let program = Calculator::parse(program.as_bytes(), limits, ObjectId::new(1, 0))?;
        let mut output = [0.0];
        program.evaluate(&[], &mut output, &mut 0, limits)?;
        Ok(output[0])
    }

    #[test]
    fn independent_calculator_arithmetic_stack_and_branches() {
        for (program, expected) in [
            ("{ 2 3 add 4 mul }", 20.0),
            ("{ -6.5 round }", -6.0),
            ("{ 90 sin }", 1.0),
            ("{ 1 2 lt {4} {9} ifelse }", 4.0),
            ("{ 1 2 2 copy add add add }", 6.0),
            ("{ 1 2 3 3 -1 roll pop pop }", 2.0),
            ("{ 5 6 1 index exch pop exch pop }", 5.0),
            ("{ -1 -1 bitshift }", 2_147_483_647.0),
            ("{ 2147483647 1 add }", 2_147_483_648.0),
            ("{ false {1 0 div} if 7 }", 7.0),
            ("{ 10 3 idiv 10 3 mod add }", 4.0),
            ("{ -2147483648 -1 mod }", 0.0),
            ("{ 3 cvr 2 exp sqrt }", 3.0),
            ("{ true false or not {0} {5} ifelse }", 5.0),
        ] {
            let output = run(program, &Limits::default()).unwrap();
            assert!((output - expected).abs() < 1e-8, "{program}");
        }
    }
    #[test]
    fn calculator_rejects_host_actions_bad_stack_and_pdf_number_syntax() {
        let limits = Limits::default();
        for source in ["{ file }", "{ {1} dup }", "{ 1e3 }", "{16#ff}"] {
            assert!(Calculator::parse(source.as_bytes(), &limits, ObjectId::new(1, 0)).is_err());
        }
        for source in [
            "{pop}",
            "{-1 sqrt}",
            "{1 0 div}",
            "{-2147483648 -1 idiv}",
            "{1 -1 copy}",
            "{true 1 and}",
            "{1 1 index}",
        ] {
            assert_eq!(
                run(source, &limits).unwrap_err().kind,
                ErrorKind::Syntax,
                "{source}"
            );
        }
        let limits = Limits {
            operations: 2,
            ..limits
        };
        assert_eq!(
            run("{1 2 add 3 mul}", &limits).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
    #[test]
    fn the_operand_stack_holds_the_annex_c_depth() {
        let limits = Limits::default();
        let fill = |count: usize| {
            let mut program = String::from("{");
            for _ in 0..count {
                program.push_str(" 1");
            }
            for _ in 1..count {
                program.push_str(" add");
            }
            program + " }"
        };
        assert_eq!(run(&fill(STACK), &limits).unwrap(), 100.0);
        assert_eq!(
            run(&fill(STACK + 1), &limits).unwrap_err().kind,
            ErrorKind::Limit
        );
        // copy past the depth is refused before copying.
        let program = format!("{{ {} {} copy }}", "1 ".repeat(60), 60);
        assert_eq!(run(&program, &limits).unwrap_err().kind, ErrorKind::Limit);
    }
}
