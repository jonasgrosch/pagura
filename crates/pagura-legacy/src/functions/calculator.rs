//! Capability-free PostScript calculator subset (7.10.5).
//! Braces compile into conditional branches, never procedure objects; only numeric/stack operators
//! execute. There is no file, network, process, name lookup or external exec.

use pagura_core::{Error, Limits, ObjectId, Position, Result};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct Calculator(Arc<[Instruction]>, ObjectId);
#[derive(Debug, Clone)]
enum Instruction {
    Push(Operand),
    Op(&'static str, usize),
    Conditional(Arc<[Instruction]>, Option<Arc<[Instruction]>>, usize),
}
#[derive(Debug, Clone)]
enum Operand {
    Integer(i32),
    Real(f64),
    Boolean(bool),
}

const OPERATORS: &[&str] = &[
    "abs", "add", "atan", "ceiling", "cos", "cvi", "cvr", "div", "exp", "floor", "idiv", "ln",
    "log", "mod", "mul", "neg", "round", "sin", "sqrt", "sub", "truncate", "and", "bitshift", "eq",
    "ge", "gt", "le", "lt", "ne", "not", "or", "xor", "copy", "dup", "exch", "index", "pop",
    "roll", "if", "ifelse",
];

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
                let yes = self.procedure(depth + 1)?;
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
                Instruction::Conditional(yes, otherwise, offset)
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
                        if let Some(&op) = OPERATORS.iter().find(|&&op| op == word) {
                            Instruction::Op(op, start)
                        } else if let Ok(n) = word.parse::<i32>() {
                            Instruction::Push(Operand::Integer(n))
                        } else {
                            let mut parser = pagura_core::syntax::Parser::new(
                                word.as_bytes(),
                                pagura_core::syntax::Origin::Buffer,
                                *self.limits,
                            );
                            let value = parser
                                .parse()
                                .ok()
                                .filter(|_| parser.at_end())
                                .and_then(|v| v.as_number())
                                .ok_or(Error::syntax(
                                    "unknown calculator operator or invalid PDF number",
                                ))?;
                            Instruction::Push(Operand::Real(value))
                        }
                    }
                }
            };
            program.push(instruction);
        }
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
        Ok(Self(program, object))
    }
    pub(super) fn evaluate(
        &self,
        input: &[f64],
        outputs: usize,
        work: &mut usize,
        limits: &Limits,
    ) -> Result<Vec<f64>> {
        let mut stack = Stack {
            values: input.iter().copied().map(Operand::Real).collect(),
            limits,
            work,
            object: self.1,
        };
        stack.execute(&self.0, 0)?;
        if stack.values.len() != outputs {
            return Err(Error::syntax("wrong calculator output count"));
        }
        stack.values.iter().map(numeric).collect()
    }
}
struct Stack<'a> {
    values: Vec<Operand>,
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
        if self.values.len() >= self.limits.value_nodes {
            return Err(Error::limit("calculator stack exceeded"));
        }
        if let Operand::Real(v) = value
            && !v.is_finite()
        {
            return Err(Error::syntax(
                "calculator arithmetic is undefined or overflows",
            ));
        }
        self.values.push(value);
        Ok(())
    }
    fn pop(&mut self) -> Result<Operand> {
        self.values
            .pop()
            .ok_or(Error::syntax("calculator stack underflow"))
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
                Instruction::Push(value) => self.push(value.clone())?,
                Instruction::Conditional(yes, otherwise, offset) => {
                    let Operand::Boolean(test) = self.pop()? else {
                        return Err(Error::syntax("calculator condition must be boolean").at(
                            Position::Stream {
                                object: self.object,
                                offset: *offset,
                            },
                        ));
                    };
                    if test {
                        self.execute(yes, depth + 1)?;
                    } else if let Some(otherwise) = otherwise {
                        self.execute(otherwise, depth + 1)?;
                    }
                }
                Instruction::Op(op, offset) => self.operator(op).map_err(|e| {
                    e.at(Position::Stream {
                        object: self.object,
                        offset: *offset,
                    })
                })?,
            }
        }
        Ok(())
    }
    fn operator(&mut self, op: &str) -> Result<()> {
        match op {
            "dup" | "exch" | "pop" | "copy" | "index" | "roll" => self.stack_operator(op),
            "and" | "or" | "xor" | "not" | "bitshift" => self.logical(op),
            "eq" | "ne" | "ge" | "gt" | "le" | "lt" => self.compare(op),
            "add" | "sub" | "mul" | "div" | "idiv" | "mod" | "exp" | "atan" => self.binary(op),
            _ => self.unary(op),
        }
    }
    fn stack_operator(&mut self, op: &str) -> Result<()> {
        match op {
            "pop" => {
                self.pop()?;
            }
            "dup" => {
                let value = self
                    .values
                    .last()
                    .cloned()
                    .ok_or(Error::syntax("calculator stack underflow"))?;
                self.push(value)?;
            }
            "exch" => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.push(a)?;
                self.push(b)?;
            }
            "copy" => {
                let count = usize::try_from(self.integer()?)
                    .map_err(|_| Error::syntax("negative calculator copy"))?;
                let start = self
                    .values
                    .len()
                    .checked_sub(count)
                    .ok_or(Error::syntax("calculator copy underflow"))?;
                if self
                    .values
                    .len()
                    .checked_add(count)
                    .is_none_or(|n| n > self.limits.value_nodes)
                {
                    return Err(Error::limit("calculator stack exceeded"));
                }
                self.charge(count)?;
                let copy = self
                    .values
                    .get(start..)
                    .ok_or(Error::syntax("calculator copy underflow"))?
                    .to_vec();
                self.values.extend(copy);
            }
            "index" => {
                let n = usize::try_from(self.integer()?)
                    .map_err(|_| Error::syntax("negative calculator index"))?;
                let index = self
                    .values
                    .len()
                    .checked_sub(n)
                    .and_then(|v| v.checked_sub(1))
                    .ok_or(Error::syntax("calculator index underflow"))?;
                self.push(
                    self.values
                        .get(index)
                        .cloned()
                        .ok_or(Error::syntax("calculator index underflow"))?,
                )?;
            }
            "roll" => {
                let shift = self.integer()?;
                let n = self.integer()?;
                if n < 0 {
                    return Err(Error::syntax("negative calculator roll"));
                }
                let count =
                    usize::try_from(n).map_err(|_| Error::limit("roll size exceeds usize"))?;
                let start = self
                    .values
                    .len()
                    .checked_sub(count)
                    .ok_or(Error::syntax("calculator roll underflow"))?;
                self.charge(count)?;
                if n > 0 {
                    let shift = usize::try_from(shift.rem_euclid(n))
                        .map_err(|_| Error::limit("roll shift exceeds usize"))?;
                    self.values
                        .get_mut(start..)
                        .ok_or(Error::syntax("calculator roll underflow"))?
                        .rotate_right(shift);
                }
            }
            _ => return Err(Error::syntax("unknown calculator stack operator")),
        }
        Ok(())
    }
    fn binary(&mut self, op: &str) -> Result<()> {
        if matches!(op, "idiv" | "mod") {
            let b = self.integer()?;
            let a = self.integer()?;
            if op == "mod" && a == i32::MIN && b == -1 {
                return self.push(Operand::Integer(0));
            }
            let result = if op == "idiv" {
                a.checked_div(b)
            } else {
                a.checked_rem(b)
            }
            .ok_or(Error::syntax("undefined calculator integer division"))?;
            return self.push(Operand::Integer(result));
        }
        let b = self.pop()?;
        let a = self.pop()?;
        if let (Operand::Integer(a), Operand::Integer(b)) = (&a, &b) {
            let result = match op {
                "add" => a.checked_add(*b),
                "sub" => a.checked_sub(*b),
                "mul" => a.checked_mul(*b),
                _ => None,
            };
            if let Some(result) = result {
                return self.push(Operand::Integer(result));
            }
        }
        let (a, b) = (numeric(&a)?, numeric(&b)?);
        let result = match op {
            "add" => a + b,
            "sub" => a - b,
            "mul" => a * b,
            "div" => {
                if b == 0.0 {
                    return Err(Error::syntax("calculator division by zero"));
                }
                a / b
            }
            "exp" => a.powf(b),
            "atan" => {
                if a == 0.0 && b == 0.0 {
                    return Err(Error::syntax("undefined calculator atan"));
                }
                a.atan2(b).to_degrees().rem_euclid(360.0)
            }
            _ => return Err(Error::syntax("unknown calculator arithmetic operator")),
        };
        self.push(Operand::Real(result))
    }
    fn unary(&mut self, op: &str) -> Result<()> {
        let value = self.pop()?;
        if let Operand::Integer(v) = value {
            if matches!(op, "floor" | "ceiling" | "round" | "truncate" | "cvi") {
                return self.push(Operand::Integer(v));
            }
            if let Some(v) = match op {
                "abs" => v.checked_abs(),
                "neg" => v.checked_neg(),
                _ => None,
            } {
                return self.push(Operand::Integer(v));
            }
        }
        let v = numeric(&value)?;
        if op == "cvi" {
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
            "abs" => v.abs(),
            "neg" => -v,
            "floor" => v.floor(),
            "ceiling" => v.ceil(),
            "round" => (v + 0.5).floor(),
            "truncate" => v.trunc(),
            "cvr" => v,
            "sqrt" => v.sqrt(),
            "ln" => {
                if v <= 0.0 {
                    return Err(Error::syntax("calculator log outside domain"));
                }
                v.ln()
            }
            "log" => {
                if v <= 0.0 {
                    return Err(Error::syntax("calculator log outside domain"));
                }
                v.log10()
            }
            "sin" => v.to_radians().sin(),
            "cos" => v.to_radians().cos(),
            _ => return Err(Error::syntax("unknown calculator unary operator")),
        };
        self.push(Operand::Real(result))
    }
    fn logical(&mut self, op: &str) -> Result<()> {
        if op == "bitshift" {
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
        if op == "not" {
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
                "and" => a & b,
                "or" => a | b,
                _ => a ^ b,
            }),
            (Operand::Boolean(a), Operand::Boolean(b)) => Operand::Boolean(match op {
                "and" => a & b,
                "or" => a | b,
                _ => a ^ b,
            }),
            _ => return Err(Error::syntax("mismatched calculator logical operands")),
        })
    }
    fn compare(&mut self, op: &str) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let equal = match (&a, &b) {
            (Operand::Boolean(a), Operand::Boolean(b)) => a == b,
            _ => numeric(&a)
                .ok()
                .zip(numeric(&b).ok())
                .is_some_and(|(a, b)| a == b),
        };
        let result = if op == "eq" {
            equal
        } else if op == "ne" {
            !equal
        } else {
            let (a, b) = (numeric(&a)?, numeric(&b)?);
            match op {
                "ge" => a >= b,
                "gt" => a > b,
                "le" => a <= b,
                "lt" => a < b,
                _ => return Err(Error::syntax("unknown calculator comparison")),
            }
        };
        self.push(Operand::Boolean(result))
    }
}
fn numeric(value: &Operand) -> Result<f64> {
    match value {
        Operand::Integer(n) => Ok(f64::from(*n)),
        Operand::Real(n) => Ok(*n),
        Operand::Boolean(_) => Err(Error::syntax("calculator requires numeric operand")),
    }
}

#[cfg(test)]
mod tests {
    use super::Calculator;
    use pagura_core::{ErrorKind, Limits, ObjectId};
    #[test]
    fn independent_calculator_arithmetic_stack_and_branches() {
        for (program, expected) in [
            ("{ 2 3 add 4 mul }", 20.0),
            ("{ -6.5 round }", -6.0),
            ("{ 90 sin }", 1.0),
            ("{ 1 2 lt {4} {9} ifelse }", 4.0),
            ("{ 1 2 2 copy add add add }", 6.0),
            ("{ 1 2 3 3 -1 roll pop pop }", 2.0),
            ("{ -1 -1 bitshift }", 2_147_483_647.0),
            ("{ 2147483647 1 add }", 2_147_483_648.0),
            ("{ false {1 0 div} if 7 }", 7.0),
            ("{ 10 3 idiv 10 3 mod add }", 4.0),
            ("{ -2147483648 -1 mod }", 0.0),
            ("{ 3 cvr 2 exp sqrt }", 3.0),
            ("{ true false or not {0} {5} ifelse }", 5.0),
        ] {
            let limits = Limits::default();
            let program =
                Calculator::parse(program.as_bytes(), &limits, ObjectId::new(1, 0)).unwrap();
            let output = program.evaluate(&[], 1, &mut 0, &limits).unwrap();
            assert!((output[0] - expected).abs() < 1e-8);
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
        ] {
            let p = Calculator::parse(source.as_bytes(), &limits, ObjectId::new(1, 0)).unwrap();
            assert_eq!(
                p.evaluate(&[], 1, &mut 0, &limits).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        let p = Calculator::parse(b"{1 2 add 3 mul}", &limits, ObjectId::new(1, 0)).unwrap();
        let limits = Limits {
            operations: 2,
            ..limits
        };
        assert_eq!(
            p.evaluate(&[], 1, &mut 0, &limits).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}
