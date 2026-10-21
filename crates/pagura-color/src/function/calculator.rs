//! The capability-free PostScript calculator subset (7.10.5).
//!
//! A program is parsed into a tree, validated statically (`check`) and
//! compiled into flat code with conditional jumps (`run`). Braces become
//! conditional branches, never procedure objects; only numeric, boolean and
//! stack operators exist, with no file, name, process or host access.

mod check;
mod run;

use pagura_core::syntax::{Origin, Parser as Numbers, is_whitespace};
use pagura_core::{Error, Limits, Position, Result};

/// A validated, compiled calculator program.
#[derive(Debug, Clone)]
pub(super) struct Program {
    code: Box<[run::Instruction]>,
    cost: usize,
}

/// An operand: the calculator's integers, reals and booleans.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    Integer(i32),
    Real(f64),
    Boolean(bool),
}

/// The operators of 7.10.5, Table 42.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
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

const OPERATORS: [(&str, Operator); 38] = [
    ("abs", Operator::Abs),
    ("add", Operator::Add),
    ("atan", Operator::Atan),
    ("ceiling", Operator::Ceiling),
    ("cos", Operator::Cos),
    ("cvi", Operator::Cvi),
    ("cvr", Operator::Cvr),
    ("div", Operator::Div),
    ("exp", Operator::Exp),
    ("floor", Operator::Floor),
    ("idiv", Operator::Idiv),
    ("ln", Operator::Ln),
    ("log", Operator::Log),
    ("mod", Operator::Mod),
    ("mul", Operator::Mul),
    ("neg", Operator::Neg),
    ("round", Operator::Round),
    ("sin", Operator::Sin),
    ("sqrt", Operator::Sqrt),
    ("sub", Operator::Sub),
    ("truncate", Operator::Truncate),
    ("and", Operator::And),
    ("bitshift", Operator::Bitshift),
    ("eq", Operator::Eq),
    ("ge", Operator::Ge),
    ("gt", Operator::Gt),
    ("le", Operator::Le),
    ("lt", Operator::Lt),
    ("ne", Operator::Ne),
    ("not", Operator::Not),
    ("or", Operator::Or),
    ("xor", Operator::Xor),
    ("copy", Operator::Copy),
    ("dup", Operator::Dup),
    ("exch", Operator::Exch),
    ("index", Operator::Index),
    ("pop", Operator::Pop),
    ("roll", Operator::Roll),
];

/// A parsed program element and its offset in the program text.
#[derive(Debug)]
enum Node {
    Push(Value, usize),
    Operator(Operator, usize),
    Conditional {
        then: Vec<Node>,
        otherwise: Option<Vec<Node>>,
        offset: usize,
    },
}

impl Program {
    /// Parses, validates and compiles `text` for `inputs` inputs and
    /// `outputs` results.
    pub(super) fn new(text: &[u8], inputs: usize, outputs: usize, limits: &Limits) -> Result<Self> {
        let mut parser = Parser {
            bytes: text,
            offset: 0,
            nodes: 0,
            limits,
        };
        let program = parser
            .procedure(0)
            .map_err(|e| e.at(Position::Buffer(parser.offset)))?;
        parser.whitespace();
        if parser.offset != text.len() {
            return Err(Error::syntax("trailing calculator program data")
                .at(Position::Buffer(parser.offset)));
        }
        let cost = check::check(&program, inputs, outputs).map_err(|e| {
            if e.position == Position::Unknown {
                e.at(Position::Buffer(text.len()))
            } else {
                e
            }
        })?;
        Ok(Self {
            code: run::compile(&program),
            cost,
        })
    }

    /// The most `function_work` units one run costs: one per instruction
    /// executed and one per value `copy` or `roll` moves.
    pub(super) fn cost(&self) -> usize {
        self.cost
    }

    /// Runs the program on `inputs` (already clipped to the domain) and
    /// writes its results to `output`.
    pub(super) fn evaluate(&self, inputs: &[f64], output: &mut [f64]) {
        run::evaluate(&self.code, inputs, output);
    }
}

/// The program text being parsed.
struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    nodes: usize,
    limits: &'a Limits,
}

impl<'a> Parser<'a> {
    /// Skips white space and comments.
    fn whitespace(&mut self) {
        loop {
            while self.bytes.get(self.offset).is_some_and(|&b| is_whitespace(b)) {
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

    /// A brace-delimited procedure.
    fn procedure(&mut self, depth: usize) -> Result<Vec<Node>> {
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
                return Ok(program);
            }
            if self.nodes >= self.limits.value_nodes {
                return Err(Error::limit("calculator instruction count exceeded"));
            }
            self.nodes += 1;
            let node = if self.bytes.get(self.offset) == Some(&b'{') {
                self.conditional(depth)?
            } else {
                self.word_node()?
            };
            program.push(node);
        }
    }

    /// `{…} if` or `{…} {…} ifelse`.
    fn conditional(&mut self, depth: usize) -> Result<Node> {
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
        Ok(Node::Conditional {
            then,
            otherwise,
            offset,
        })
    }

    /// An operator, a boolean or a number.
    fn word_node(&mut self) -> Result<Node> {
        let (word, start) = self.word()?;
        Ok(match word {
            "true" => Node::Push(Value::Boolean(true), start),
            "false" => Node::Push(Value::Boolean(false), start),
            "if" | "ifelse" => {
                return Err(Error::syntax(
                    "calculator conditional requires brace expressions",
                ));
            }
            _ => {
                if let Some(&(_, operator)) = OPERATORS.iter().find(|(name, _)| *name == word) {
                    Node::Operator(operator, start)
                } else if let Ok(n) = word.parse::<i32>() {
                    Node::Push(Value::Integer(n), start)
                } else {
                    Node::Push(Value::Real(self.number(word)?), start)
                }
            }
        })
    }

    /// A PDF real number (7.3.3); PostScript radix and exponent forms are
    /// not numbers here.
    fn number(&self, word: &str) -> Result<f64> {
        let mut numbers = Numbers::new(word.as_bytes(), Origin::Buffer, *self.limits);
        numbers
            .parse()
            .ok()
            .filter(|_| numbers.at_end())
            .and_then(|v| v.as_number())
            .ok_or(Error::syntax(
                "unknown calculator operator or invalid PDF number",
            ))
    }

    /// The next token and its offset.
    fn word(&mut self) -> Result<(&'a str, usize)> {
        self.whitespace();
        let start = self.offset;
        while self
            .bytes
            .get(self.offset)
            .is_some_and(|&b| !is_whitespace(b) && !matches!(b, b'{' | b'}' | b'%'))
        {
            self.offset += 1;
        }
        if start == self.offset {
            return Err(Error::syntax("unterminated calculator expression"));
        }
        let bytes = self
            .bytes
            .get(start..self.offset)
            .ok_or(Error::syntax("invalid calculator token"))?;
        let word =
            std::str::from_utf8(bytes).map_err(|_| Error::syntax("invalid calculator token"))?;
        Ok((word, start))
    }
}
