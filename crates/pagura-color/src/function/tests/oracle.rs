//! The previous function evaluation (`pagura-render` `functions.rs`,
//! `functions/sampled.rs` and `functions/calculator.rs` at the start of the
//! one-pass refactor), ported to the new parameter structs as a test-only
//! differential oracle. Its arithmetic, branch selection, error returns and
//! work counting are kept as they were; only document parsing is replaced.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the ported casts are bounded as they were in the previous code"
)]

use crate::function::{CalculatorFunction, ExponentialFunction, SampledFunction};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A function as the previous code held it.
#[derive(Debug, Clone)]
pub(crate) struct Oracle {
    domain: Vec<[f64; 2]>,
    range: Option<Vec<[f64; 2]>>,
    outputs: usize,
    body: Body,
    limits: Limits,
    unit_bytes: bool,
}

#[derive(Debug, Clone)]
enum Body {
    Exponential {
        low: Vec<f64>,
        high: Vec<f64>,
        power: f64,
    },
    Stitch {
        functions: Vec<Oracle>,
        bounds: Vec<f64>,
        encode: Vec<[f64; 2]>,
    },
    Sampled(Sampled),
    Calculator(Calculator),
}

/// Budgets large enough that the oracle never runs out of work.
pub(crate) fn unlimited() -> Limits {
    Limits {
        operations: usize::MAX / 2,
        ..Limits::default()
    }
}

impl Oracle {
    pub(crate) fn exponential(p: &ExponentialFunction) -> Self {
        Self {
            domain: vec![p.domain],
            range: p.range.clone(),
            outputs: p.c0.len(),
            body: Body::Exponential {
                low: p.c0.clone(),
                high: p.c1.clone(),
                power: p.exponent,
            },
            limits: unlimited(),
            unit_bytes: false,
        }
    }

    pub(crate) fn stitching(
        domain: [f64; 2],
        range: Option<Vec<[f64; 2]>>,
        functions: Vec<Self>,
        bounds: Vec<f64>,
        encode: Vec<[f64; 2]>,
    ) -> Self {
        Self {
            domain: vec![domain],
            range,
            outputs: functions[0].outputs,
            body: Body::Stitch {
                functions,
                bounds,
                encode,
            },
            limits: unlimited(),
            unit_bytes: false,
        }
    }

    pub(crate) fn sampled(p: &SampledFunction) -> Self {
        let size: Vec<usize> = p.size.iter().map(|&n| n as usize).collect();
        let outputs = p.range.len();
        let mut count = outputs;
        for &n in &size {
            count *= n;
        }
        let bits = u32::from(p.bits_per_sample);
        let required = (count * bits as usize).div_ceil(8);
        let defaults: Vec<_> = size.iter().map(|&n| [0.0, (n - 1) as f64]).collect();
        let sampled = Sampled {
            size,
            bits,
            cubic: p.order == 3,
            data: p.samples[..required].into(),
            encode: p.encode.clone().unwrap_or(defaults),
            decode: p.decode.clone().unwrap_or_else(|| p.range.clone()),
        };
        let unit_bytes = p.domain == [[0.0, 1.0]] && sampled.unit_byte_grid();
        Self {
            domain: p.domain.clone(),
            range: Some(p.range.clone()),
            outputs,
            body: Body::Sampled(sampled),
            limits: unlimited(),
            unit_bytes,
        }
    }

    /// The previous parser; its errors are the previous parse errors.
    pub(crate) fn calculator(p: &CalculatorFunction, limits: &Limits) -> Result<Self> {
        Ok(Self {
            domain: p.domain.clone(),
            range: Some(p.range.clone()),
            outputs: p.range.len(),
            body: Body::Calculator(Calculator::parse(&p.program, limits)?),
            limits: unlimited(),
            unit_bytes: false,
        })
    }

    /// `Function::evaluate_into`, as shadings, transfers and BG/UCR called it.
    pub(crate) fn evaluate_into(
        &self,
        inputs: &[f64],
        output: &mut [f64],
        work: &mut usize,
    ) -> Result<()> {
        if inputs.len() != self.domain.len() || inputs.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax("invalid function inputs"));
        }
        if output.len() != self.outputs {
            return Err(Error::invalid_argument("wrong function output buffer"));
        }
        if let Body::Exponential { low, high, power } = &self.body {
            *work += 1;
            let [value] = inputs else {
                return Err(Error::syntax("wrong function input count"));
            };
            let range = self.domain[0];
            let value = value.clamp(range[0], range[1]).powf(*power);
            for (i, ((out, &low), &high)) in output.iter_mut().zip(low).zip(high).enumerate() {
                *out = low + value * (high - low);
                if !out.is_finite() {
                    return Err(Error::syntax(
                        "function arithmetic is undefined or overflows",
                    ));
                }
                if let Some(range) = self.range.as_ref().and_then(|r| r.get(i)) {
                    *out = out.clamp(range[0], range[1]);
                }
            }
            return Ok(());
        }
        self.run_into(inputs, output, 0, work)
    }

    /// `Function::evaluate_with_work`, as tint transforms called it.
    pub(crate) fn evaluate_with_work(&self, inputs: &[f64], work: &mut usize) -> Result<Vec<f64>> {
        let mut output = vec![0.0; self.outputs];
        self.run_into(inputs, &mut output, 0, work)?;
        Ok(output)
    }

    fn run_into(
        &self,
        inputs: &[f64],
        output: &mut [f64],
        depth: usize,
        work: &mut usize,
    ) -> Result<()> {
        if depth >= self.limits.depth {
            return Err(Error::limit("function evaluation budget exceeded"));
        }
        *work += 1;
        if inputs.len() != self.domain.len() || inputs.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax("invalid function inputs"));
        }
        let first = || -> f64 {
            let range = self.domain[0];
            inputs[0].clamp(range[0], range[1])
        };
        match &self.body {
            Body::Exponential { low, high, power } => {
                let x = first().powf(*power);
                for ((out, &a), &b) in output.iter_mut().zip(low).zip(high) {
                    *out = a + x * (b - a);
                }
            }
            Body::Stitch {
                functions,
                bounds,
                encode,
            } => {
                let x = first();
                let index = if bounds.first() == Some(&x) && self.domain[0][0] == x {
                    0
                } else {
                    bounds.partition_point(|&b| x >= b)
                };
                let domain = self.domain[0];
                let low = if index == 0 {
                    domain[0]
                } else {
                    bounds[index - 1]
                };
                let high = bounds.get(index).copied().unwrap_or(domain[1]);
                let x = interpolate(x, [low, high], encode[index]);
                functions[index].run_into(&[x], output, depth + 1, work)?;
            }
            Body::Calculator(calculator) => {
                let input: Vec<_> = inputs
                    .iter()
                    .zip(&self.domain)
                    .map(|(&v, r)| v.clamp(r[0], r[1]))
                    .collect();
                let result = calculator.evaluate(&input, self.outputs, work, &self.limits)?;
                if result.len() != output.len() {
                    return Err(Error::syntax(
                        "function arithmetic is undefined or overflows",
                    ));
                }
                output.copy_from_slice(&result);
            }
            Body::Sampled(sampled) if self.unit_bytes => {
                let size = sampled.size[0];
                sampled.interpolate_bytes(first() * (size - 1) as f64, output, work)?;
            }
            Body::Sampled(sampled) => {
                sampled.evaluate_into(inputs, &self.domain, output, work, &self.limits)?;
            }
        }
        if output.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax(
                "function arithmetic is undefined or overflows",
            ));
        }
        if let Some(range) = &self.range {
            for (v, r) in output.iter_mut().zip(range) {
                *v = v.clamp(r[0], r[1]);
            }
        }
        Ok(())
    }
}

fn interpolate(value: f64, from: [f64; 2], to: [f64; 2]) -> f64 {
    if from[0] == from[1] {
        return to[0];
    }
    to[0] + ((value - from[0]) / (from[1] - from[0])) * (to[1] - to[0])
}

#[derive(Debug, Clone)]
struct Sampled {
    size: Vec<usize>,
    bits: u32,
    cubic: bool,
    data: Arc<[u8]>,
    encode: Vec<[f64; 2]>,
    decode: Vec<[f64; 2]>,
}

impl Sampled {
    fn linear_bytes(&self) -> bool {
        self.bits == 8 && self.size.len() == 1 && (!self.cubic || self.size[0] < 4)
    }

    fn unit_byte_grid(&self) -> bool {
        self.linear_bytes()
            && self.encode == [[0.0, (self.size[0] - 1) as f64]]
            && self.decode.iter().all(|&r| r == [0.0, 1.0])
    }

    fn evaluate_into(
        &self,
        input: &[f64],
        domain: &[[f64; 2]],
        result: &mut [f64],
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        if self.linear_bytes()
            && let ([x], [domain], [encode], [size]) = (input, domain, &*self.encode, &*self.size)
        {
            let x = interpolate(x.clamp(domain[0], domain[1]), *domain, *encode)
                .clamp(0.0, (size - 1) as f64);
            if !x.is_finite() {
                return Err(Error::syntax("sample encode arithmetic overflow"));
            }
            self.interpolate_bytes(x, result, work)?;
            for (value, range) in result.iter_mut().zip(&self.decode) {
                *value = interpolate(*value, [0.0, 1.0], *range);
            }
            return Ok(());
        }
        let mut positions = vec![Weights::EMPTY; input.len()];
        let outputs = result.len();
        let mut vertices = 1_usize;
        for (slot, (((&x, domain), encode), &size)) in positions
            .iter_mut()
            .zip(input.iter().zip(domain).zip(&self.encode).zip(&self.size))
        {
            let x = interpolate(x.clamp(domain[0], domain[1]), *domain, *encode)
                .clamp(0.0, (size - 1) as f64);
            if !x.is_finite() {
                return Err(Error::syntax("sample encode arithmetic overflow"));
            }
            let position = Weights::new(x, size, self.cubic);
            vertices = vertices
                .checked_mul(position.len)
                .filter(|&n| n <= limits.operations)
                .ok_or(Error::limit("sampled interpolation corner budget exceeded"))?;
            *slot = position;
        }
        result.fill(0.0);
        for corner in 0..vertices {
            *work += input.len() + outputs;
            let (mut offset, mut stride, mut weight) = (0_usize, outputs, 1.0);
            let mut remaining = corner;
            for (position, &size) in positions.iter().zip(&self.size) {
                let (index, factor) = position.values[remaining % position.len];
                remaining /= position.len;
                offset += index * stride;
                stride *= size;
                weight *= factor;
            }
            if weight == 0.0 {
                continue;
            }
            for (component, value) in result.iter_mut().enumerate() {
                *value += weight * self.sample(offset + component)?;
            }
        }
        for (value, range) in result.iter_mut().zip(&self.decode) {
            *value = interpolate(*value, [0.0, 1.0], *range);
        }
        Ok(())
    }

    fn interpolate_bytes(&self, x: f64, output: &mut [f64], work: &mut usize) -> Result<()> {
        let size = self.size[0];
        let low = x.floor() as usize;
        let t = x - low as f64;
        let corners = if t == 0.0 { 1 } else { 2 };
        *work += (1 + output.len()) * corners;
        let tuple = |index: usize| -> Result<&[u8]> {
            let start = index * output.len();
            self.data
                .get(start..start + output.len())
                .ok_or(Error::syntax("truncated sampled function"))
        };
        let a = tuple(low)?;
        if t == 0.0 {
            for (out, &a) in output.iter_mut().zip(a) {
                *out = f64::from(a) / 255.0;
            }
        } else {
            let b = tuple((low + 1).min(size - 1))?;
            for ((out, &a), &b) in output.iter_mut().zip(a).zip(b) {
                *out = (1.0 - t) * (f64::from(a) / 255.0) + t * (f64::from(b) / 255.0);
            }
        }
        Ok(())
    }

    fn sample(&self, index: usize) -> Result<f64> {
        let width = self.bits as usize;
        let start = index * width;
        let truncated = || Error::syntax("truncated sampled function");
        let value = match width {
            1 | 2 | 4 => {
                let byte = *self.data.get(start / 8).ok_or_else(truncated)?;
                u64::from((byte >> (8 - width - start % 8)) & ((1 << width) - 1))
            }
            12 => {
                let Some(&[high, low]) = self.data.get(start / 8..start / 8 + 2) else {
                    return Err(truncated());
                };
                u64::from(u16::from_be_bytes([high, low]) >> (4 - start % 8)) & 0xfff
            }
            _ => self
                .data
                .get(start / 8..start / 8 + width / 8)
                .ok_or_else(truncated)?
                .iter()
                .fold(0_u64, |value, &byte| (value << 8) | u64::from(byte)),
        };
        Ok(value as f64 / ((1_u64 << self.bits) - 1) as f64)
    }
}

#[derive(Clone, Copy)]
struct Weights {
    values: [(usize, f64); 4],
    len: usize,
}

impl Weights {
    const EMPTY: Self = Self {
        values: [(0, 0.0); 4],
        len: 0,
    };

    fn new(x: f64, size: usize, cubic: bool) -> Self {
        let low = x.floor() as usize;
        let t = x - low as f64;
        let high = (low + 1).min(size - 1);
        if size == 1 || t == 0.0 {
            return Self {
                values: [(low, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
                len: 1,
            };
        }
        if !cubic || size < 4 {
            return Self {
                values: [(low, 1.0 - t), (high, t), (0, 0.0), (0, 0.0)],
                len: 2,
            };
        }
        let t2 = t * t;
        let t3 = t2 * t;
        let mut values = [
            (low.saturating_sub(1), -0.5 * t + t2 - 0.5 * t3),
            (low, 1.0 - 2.5 * t2 + 1.5 * t3),
            (high, 0.5 * t + 2.0 * t2 - 1.5 * t3),
            ((high + 1).min(size - 1), -0.5 * t2 + 0.5 * t3),
        ];
        if low == 0 {
            values[1].1 += 2.0 * values[0].1;
            values[2].1 -= values[0].1;
            values[0].1 = 0.0;
        }
        if high == size - 1 {
            values[1].1 -= values[3].1;
            values[2].1 += 2.0 * values[3].1;
            values[3].1 = 0.0;
        }
        Self { values, len: 4 }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Calculator(Arc<[Instruction]>);

#[derive(Debug, Clone)]
enum Instruction {
    Push(Operand),
    Op(&'static str),
    Conditional(Arc<[Instruction]>, Option<Arc<[Instruction]>>),
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
                let yes = self.procedure(depth + 1)?;
                self.whitespace();
                let otherwise = if self.bytes.get(self.offset) == Some(&b'{') {
                    Some(self.procedure(depth + 1)?)
                } else {
                    None
                };
                let word = self.word()?;
                if word != if otherwise.is_some() { "ifelse" } else { "if" } {
                    return Err(Error::syntax(
                        "calculator brace expression requires conditional operator",
                    ));
                }
                Instruction::Conditional(yes, otherwise)
            } else {
                let word = self.word()?;
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
                            Instruction::Op(op)
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

    fn word(&mut self) -> Result<&'a str> {
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
        std::str::from_utf8(&self.bytes[start..self.offset])
            .map_err(|_| Error::syntax("invalid calculator token"))
    }
}

impl Calculator {
    pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        let mut parser = Parser {
            bytes,
            offset: 0,
            nodes: 0,
            limits,
        };
        let program = parser.procedure(0)?;
        parser.whitespace();
        if parser.offset != bytes.len() {
            return Err(Error::syntax("trailing calculator program data"));
        }
        Ok(Self(program))
    }

    fn evaluate(
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
}

impl Stack<'_> {
    fn charge(&mut self, count: usize) {
        *self.work += count;
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
            self.charge(1);
            match instruction {
                Instruction::Push(value) => self.push(value.clone())?,
                Instruction::Conditional(yes, otherwise) => {
                    let Operand::Boolean(test) = self.pop()? else {
                        return Err(Error::syntax("calculator condition must be boolean"));
                    };
                    if test {
                        self.execute(yes, depth + 1)?;
                    } else if let Some(otherwise) = otherwise {
                        self.execute(otherwise, depth + 1)?;
                    }
                }
                Instruction::Op(op) => self.operator(op)?,
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
                self.charge(count);
                let copy = self.values[start..].to_vec();
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
                self.push(self.values[index].clone())?;
            }
            "roll" => {
                let shift = self.integer()?;
                let n = self.integer()?;
                if n < 0 {
                    return Err(Error::syntax("negative calculator roll"));
                }
                let count = n as usize;
                let start = self
                    .values
                    .len()
                    .checked_sub(count)
                    .ok_or(Error::syntax("calculator roll underflow"))?;
                self.charge(count);
                if n > 0 {
                    let shift = shift.rem_euclid(n) as usize;
                    self.values[start..].rotate_right(shift);
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
