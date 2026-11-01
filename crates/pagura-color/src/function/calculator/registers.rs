//! Calculator programs compiled to straight-line code over real registers.
//!
//! Validation fixes every stack effect: the counts of `copy`, `index` and
//! `roll` are known. A program whose shifts and conditions are known as well,
//! and in which every value that depends on the inputs is a real, then runs
//! the same arithmetic on every run, in the same order: compiling it runs the
//! program once over stack entries that are either a value the same in every
//! run or a real in a register. Stack operators only move entries, operators
//! on known values are evaluated while compiling (by the interpreter's own
//! stack), and what is left are real operators on registers, computed by the
//! interpreter's functions and made finite as a push makes them. Other
//! programs, and programs that need more registers, run interpreted.

use super::super::make_finite;
use super::run::{self, Stack};
use super::{Node, Operator, Value};

/// The registers compiled code may use.
const REGISTERS: usize = 32;

/// Compiled straight-line code.
#[derive(Debug, Clone)]
pub(super) struct Compiled {
    steps: Box<[Step]>,
    /// The results, bottom of the stack first.
    outputs: Box<[Operand]>,
}

/// An operand: a register, or a number known while compiling.
#[derive(Debug, Clone, Copy)]
enum Operand {
    Register(u8),
    Constant(f64),
}

/// One real operator, written to register `to`.
#[derive(Debug, Clone, Copy)]
enum Step {
    Binary {
        operator: Operator,
        to: u8,
        a: Operand,
        b: Operand,
    },
    Unary {
        operator: Operator,
        to: u8,
        value: Operand,
    },
}

/// A stack entry while compiling.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// The same value in every run.
    Known(Value),
    /// A real held in a register.
    Real(u8),
}

/// Compiles `program`, run on `inputs` real operands, where it qualifies.
pub(super) fn compile(program: &[Node], inputs: usize) -> Option<Compiled> {
    let mut compiler = Compiler {
        entries: Vec::new(),
        uses: [0; REGISTERS],
        steps: Vec::new(),
    };
    for register in 0..inputs {
        let register = u8::try_from(register)
            .ok()
            .filter(|&r| usize::from(r) < REGISTERS)?;
        compiler.uses[usize::from(register)] = 1;
        compiler.entries.push(Entry::Real(register));
    }
    compiler.run(program)?;
    let outputs = compiler
        .entries
        .iter()
        .map(|entry| match *entry {
            Entry::Known(value) => Operand::Constant(value.number()),
            Entry::Real(register) => Operand::Register(register),
        })
        .collect();
    Some(Compiled {
        steps: compiler.steps.into(),
        outputs,
    })
}

impl Compiled {
    /// Bytes held besides the value.
    pub(super) fn heap_bytes(&self) -> usize {
        size_of_val(self.steps.as_ref()) + size_of_val(self.outputs.as_ref())
    }

    /// Runs the code on `inputs` and writes the results to `output`, as
    /// [`run::evaluate`] runs the program.
    pub(super) fn evaluate(&self, inputs: &[f64], output: &mut [f64]) {
        let mut registers = [0.0; REGISTERS];
        for (register, &input) in registers.iter_mut().zip(inputs) {
            *register = make_finite(input);
        }
        let read = |registers: &[f64; REGISTERS], operand: Operand| match operand {
            Operand::Register(r) => registers.get(usize::from(r)).copied().unwrap_or(0.0),
            Operand::Constant(value) => value,
        };
        for &step in &self.steps {
            let (to, value) = match step {
                Step::Binary { operator, to, a, b } => (
                    to,
                    run::real_binary(operator, read(&registers, a), read(&registers, b)),
                ),
                Step::Unary {
                    operator,
                    to,
                    value,
                } => (to, run::real_unary(operator, read(&registers, value))),
            };
            if let Some(register) = registers.get_mut(usize::from(to)) {
                *register = make_finite(value);
            }
        }
        for (out, &operand) in output.iter_mut().zip(&self.outputs) {
            *out = read(&registers, operand);
        }
    }
}

/// The stack entries and register uses at one point of compilation.
struct Compiler {
    entries: Vec<Entry>,
    /// How many entries refer to each register.
    uses: [u16; REGISTERS],
    steps: Vec<Step>,
}

impl Compiler {
    /// Compiles `program`; `None` where it does not qualify.
    fn run(&mut self, program: &[Node]) -> Option<()> {
        for node in program {
            match node {
                Node::Push(value, _) => self.entries.push(Entry::Known(run::pushed(*value))),
                Node::Operator(operator, _) => self.operator(*operator)?,
                Node::Conditional {
                    then, otherwise, ..
                } => match self.pop()? {
                    Entry::Known(Value::Boolean(true)) => self.run(then)?,
                    Entry::Known(Value::Boolean(false)) => {
                        if let Some(otherwise) = otherwise {
                            self.run(otherwise)?;
                        }
                    }
                    _ => return None,
                },
            }
        }
        Some(())
    }

    fn operator(&mut self, operator: Operator) -> Option<()> {
        use Operator as O;
        match operator {
            O::Pop => {
                self.pop()?;
            }
            O::Dup => self.copy(1)?,
            O::Exch => {
                let len = self.entries.len();
                self.entries.get_mut(len.checked_sub(2)?..)?.swap(0, 1);
            }
            O::Copy => {
                let count = self.count()?;
                self.copy(count)?;
            }
            O::Index => {
                let n = self.count()?;
                let at = self.entries.len().checked_sub(n)?.checked_sub(1)?;
                let entry = *self.entries.get(at)?;
                self.push(entry);
            }
            O::Roll => {
                let shift = self.integer()?;
                let n = self.integer()?;
                let count = usize::try_from(n).ok()?;
                let start = self.entries.len().checked_sub(count)?;
                if n > 0 {
                    let shift = usize::try_from(shift.rem_euclid(n)).ok()?;
                    self.entries.get_mut(start..)?.rotate_right(shift);
                }
            }
            O::Add | O::Sub | O::Mul | O::Div | O::Exp | O::Atan => {
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Entry::Known(a), Entry::Known(b)) = (a, b) {
                    self.fold(operator, &[a, b]);
                    return Some(());
                }
                // A real operand takes the operator to its real arithmetic.
                let (a, b) = (operand(a)?, operand(b)?);
                let to = self.register()?;
                self.steps.push(Step::Binary { operator, to, a, b });
            }
            O::Abs
            | O::Neg
            | O::Floor
            | O::Ceiling
            | O::Round
            | O::Truncate
            | O::Cvr
            | O::Sqrt
            | O::Ln
            | O::Log
            | O::Sin
            | O::Cos => match self.pop()? {
                Entry::Known(value) => self.fold(operator, &[value]),
                Entry::Real(register) => {
                    let to = self.register()?;
                    self.steps.push(Step::Unary {
                        operator,
                        to,
                        value: Operand::Register(register),
                    });
                }
            },
            // Integer and boolean results of reals that change from run to
            // run are not registers' values.
            O::Not | O::Cvi => match self.pop()? {
                Entry::Known(value) => self.fold(operator, &[value]),
                Entry::Real(_) => return None,
            },
            O::Idiv
            | O::Mod
            | O::Bitshift
            | O::And
            | O::Or
            | O::Xor
            | O::Eq
            | O::Ne
            | O::Ge
            | O::Gt
            | O::Le
            | O::Lt => {
                let b = self.pop()?;
                let a = self.pop()?;
                let (Entry::Known(a), Entry::Known(b)) = (a, b) else {
                    return None;
                };
                self.fold(operator, &[a, b]);
            }
        }
        Some(())
    }

    /// Pushes the result of `operator` on known `operands`, as the
    /// interpreter computes it.
    fn fold(&mut self, operator: Operator, operands: &[Value]) {
        let mut stack = Stack::new();
        for &value in operands {
            stack.push(value);
        }
        stack.apply(operator);
        self.entries.push(Entry::Known(stack.pop()));
    }

    /// Copies the top `count` entries.
    fn copy(&mut self, count: usize) -> Option<()> {
        let start = self.entries.len().checked_sub(count)?;
        for at in start..start + count {
            let entry = *self.entries.get(at)?;
            self.push(entry);
        }
        Some(())
    }

    fn push(&mut self, entry: Entry) {
        if let Entry::Real(register) = entry
            && let Some(uses) = self.uses.get_mut(usize::from(register))
        {
            *uses += 1;
        }
        self.entries.push(entry);
    }

    fn pop(&mut self) -> Option<Entry> {
        let entry = self.entries.pop()?;
        if let Entry::Real(register) = entry
            && let Some(uses) = self.uses.get_mut(usize::from(register))
        {
            *uses = uses.saturating_sub(1);
        }
        Some(entry)
    }

    /// A known integer operand: a count or a shift.
    fn integer(&mut self) -> Option<i32> {
        match self.pop()? {
            Entry::Known(Value::Integer(n)) => Some(n),
            _ => None,
        }
    }

    /// A known count of entries.
    fn count(&mut self) -> Option<usize> {
        usize::try_from(self.integer()?).ok()
    }

    /// A free register for a new real, pushed as the top entry. The
    /// operands it replaces are read before it is written, so it may be one
    /// of theirs.
    fn register(&mut self) -> Option<u8> {
        let free = self.uses.iter().position(|&uses| uses == 0)?;
        let register = u8::try_from(free).ok()?;
        self.push(Entry::Real(register));
        Some(register)
    }
}

/// An entry as an operand of a real operator: a known number or a register;
/// `None` for a boolean.
fn operand(entry: Entry) -> Option<Operand> {
    match entry {
        Entry::Real(register) => Some(Operand::Register(register)),
        Entry::Known(Value::Boolean(_)) => None,
        Entry::Known(value) => Some(Operand::Constant(value.number())),
    }
}
