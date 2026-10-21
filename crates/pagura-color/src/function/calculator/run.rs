//! Compiled calculator code and its evaluation. Validation has proved that
//! every run has the operands each instruction needs, of the right kind, so
//! evaluation has no failure paths; the fallbacks below for missing or
//! mistyped operands are unreachable and only keep the code panic free.

use super::super::make_finite;
use super::{Node, Operator, Value};
use pagura_core::Limits;

/// One instruction of flat code.
#[derive(Debug, Clone, Copy)]
pub(super) enum Instruction {
    Push(Value),
    Operator(Operator),
    /// Pops a boolean and continues at the target when it is false.
    Unless(usize),
    /// Continues at the target.
    Jump(usize),
}

/// Compiles a validated program tree into flat code.
pub(super) fn compile(program: &[Node]) -> Box<[Instruction]> {
    let mut code = Vec::new();
    emit(program, &mut code);
    code.into()
}

fn emit(program: &[Node], code: &mut Vec<Instruction>) {
    for node in program {
        match node {
            Node::Push(value, _) => code.push(Instruction::Push(*value)),
            Node::Operator(operator, _) => code.push(Instruction::Operator(*operator)),
            Node::Conditional {
                then, otherwise, ..
            } => {
                let unless = code.len();
                code.push(Instruction::Unless(0));
                emit(then, code);
                if let Some(otherwise) = otherwise {
                    let jump = code.len();
                    code.push(Instruction::Jump(0));
                    patch(code, unless);
                    emit(otherwise, code);
                    patch(code, jump);
                } else {
                    patch(code, unless);
                }
            }
        }
    }
}

/// Points the jump at `at` to the end of the code emitted so far.
fn patch(code: &mut [Instruction], at: usize) {
    let end = code.len();
    if let Some(Instruction::Unless(target) | Instruction::Jump(target)) = code.get_mut(at) {
        *target = end;
    }
}

/// Runs `code` on `inputs` and writes the stack, bottom first, to `output`.
pub(super) fn evaluate(code: &[Instruction], inputs: &[f64], output: &mut [f64]) {
    let mut stack = Stack {
        values: [Value::Integer(0); Limits::CALCULATOR_STACK],
        len: 0,
    };
    for &input in inputs {
        stack.push(Value::Real(input));
    }
    let mut next = 0;
    while let Some(&instruction) = code.get(next) {
        next += 1;
        match instruction {
            Instruction::Push(value) => stack.push(value),
            Instruction::Operator(operator) => stack.apply(operator),
            Instruction::Unless(target) => {
                if stack.pop() != Value::Boolean(true) {
                    next = target;
                }
            }
            Instruction::Jump(target) => next = target,
        }
    }
    for (out, value) in output.iter_mut().zip(stack.slice()) {
        *out = value.number();
    }
}

/// `add`, `sub` and `mul` on integers; `None` when the result overflows.
pub(super) fn checked(operator: Operator, a: i32, b: i32) -> Option<i32> {
    match operator {
        Operator::Add => a.checked_add(b),
        Operator::Sub => a.checked_sub(b),
        Operator::Mul => a.checked_mul(b),
        _ => None,
    }
}

/// The integer operators with integer results: `idiv`, `mod`, `bitshift`,
/// `and`, `or` and `xor`, with their defined results for faults.
pub(super) fn integer(operator: Operator, a: i32, b: i32) -> i32 {
    match operator {
        Operator::Idiv if b == 0 => 0,
        Operator::Idiv => a.saturating_div(b),
        // Covers division by zero and −2³¹ mod −1, both 0.
        Operator::Mod => a.checked_rem(b).unwrap_or(0),
        Operator::Bitshift => {
            let value = a.cast_unsigned();
            let amount = b.unsigned_abs();
            let result = if amount >= 32 {
                0
            } else if b >= 0 {
                value << amount
            } else {
                value >> amount
            };
            result.cast_signed()
        }
        Operator::And => a & b,
        Operator::Or => a | b,
        _ => a ^ b,
    }
}

impl Value {
    /// The value as a real; a boolean (never a validated result) reads as 0.
    fn number(self) -> f64 {
        match self {
            Self::Integer(n) => f64::from(n),
            Self::Real(v) => v,
            Self::Boolean(_) => 0.0,
        }
    }

    /// The value where an integer is required: a real (an integer result
    /// that overflowed) is truncated and saturated.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "saturating conversion is the defined result"
    )]
    fn integer(self) -> i32 {
        match self {
            Self::Integer(n) => n,
            Self::Real(v) => v as i32,
            Self::Boolean(_) => 0,
        }
    }
}

/// The operand stack of one run.
struct Stack {
    values: [Value; Limits::CALCULATOR_STACK],
    len: usize,
}

impl Stack {
    fn slice(&self) -> &[Value] {
        self.values.get(..self.len).unwrap_or(&[])
    }

    fn push(&mut self, value: Value) {
        let value = match value {
            Value::Real(v) => Value::Real(make_finite(v)),
            other => other,
        };
        if let Some(slot) = self.values.get_mut(self.len) {
            *slot = value;
            self.len += 1;
        }
    }

    fn pop(&mut self) -> Value {
        if let Some(len) = self.len.checked_sub(1) {
            self.len = len;
            self.values.get(len).copied().unwrap_or(Value::Integer(0))
        } else {
            Value::Integer(0)
        }
    }

    fn real(&mut self, value: f64) {
        self.push(Value::Real(value));
    }

    fn apply(&mut self, operator: Operator) {
        use Operator as O;
        match operator {
            O::Pop | O::Dup | O::Exch | O::Copy | O::Index | O::Roll => self.stack(operator),
            O::Add | O::Sub | O::Mul | O::Div | O::Exp | O::Atan => self.binary(operator),
            O::Idiv | O::Mod | O::Bitshift => {
                let b = self.pop().integer();
                let a = self.pop().integer();
                self.push(Value::Integer(integer(operator, a, b)));
            }
            O::And | O::Or | O::Xor => {
                let b = self.pop();
                let a = self.pop();
                let value = if let (Value::Boolean(a), Value::Boolean(b)) = (a, b) {
                    Value::Boolean(match operator {
                        O::And => a & b,
                        O::Or => a | b,
                        _ => a ^ b,
                    })
                } else {
                    Value::Integer(integer(operator, a.integer(), b.integer()))
                };
                self.push(value);
            }
            O::Not => {
                let value = match self.pop() {
                    Value::Boolean(v) => Value::Boolean(!v),
                    other => Value::Integer(!other.integer()),
                };
                self.push(value);
            }
            O::Eq | O::Ne | O::Ge | O::Gt | O::Le | O::Lt => self.compare(operator),
            _ => self.unary(operator),
        }
    }

    fn stack(&mut self, operator: Operator) {
        match operator {
            Operator::Pop => {
                self.pop();
            }
            Operator::Dup => {
                let top = self.slice().last().copied().unwrap_or(Value::Integer(0));
                self.push(top);
            }
            Operator::Exch => {
                let a = self.pop();
                let b = self.pop();
                self.push(a);
                self.push(b);
            }
            Operator::Copy => {
                let count = usize::try_from(self.pop().integer()).unwrap_or(0);
                let start = self.len.saturating_sub(count);
                for i in start..start + count {
                    let value = self.values.get(i).copied().unwrap_or(Value::Integer(0));
                    self.push(value);
                }
            }
            Operator::Index => {
                let n = usize::try_from(self.pop().integer()).unwrap_or(0);
                let index = self.len.saturating_sub(n).saturating_sub(1);
                let value = self.values.get(index).copied().unwrap_or(Value::Integer(0));
                self.push(value);
            }
            _ => {
                let shift = self.pop().integer();
                let n = self.pop().integer();
                let count = usize::try_from(n).unwrap_or(0);
                let start = self.len.saturating_sub(count);
                if n > 0
                    && let Some(rolled) = self.values.get_mut(start..self.len)
                {
                    rolled.rotate_right(usize::try_from(shift.rem_euclid(n)).unwrap_or(0));
                }
            }
        }
    }

    fn binary(&mut self, operator: Operator) {
        let b = self.pop();
        let a = self.pop();
        if let (Value::Integer(a), Value::Integer(b)) = (a, b)
            && let Some(result) = checked(operator, a, b)
        {
            self.push(Value::Integer(result));
            return;
        }
        let (a, b) = (a.number(), b.number());
        self.real(match operator {
            Operator::Add => a + b,
            Operator::Sub => a - b,
            Operator::Mul => a * b,
            Operator::Div => a / b,
            Operator::Exp => a.powf(b),
            _ if a == 0.0 && b == 0.0 => 0.0,
            _ => a.atan2(b).to_degrees().rem_euclid(360.0),
        });
    }

    fn unary(&mut self, operator: Operator) {
        use Operator as O;
        let value = self.pop();
        if let Value::Integer(v) = value {
            let result = match operator {
                O::Floor | O::Ceiling | O::Round | O::Truncate | O::Cvi => Some(v),
                O::Abs => v.checked_abs(),
                O::Neg => v.checked_neg(),
                _ => None,
            };
            if let Some(result) = result {
                self.push(Value::Integer(result));
                return;
            }
        }
        let v = value.number();
        if operator == O::Cvi {
            self.push(Value::Integer(Value::Real(v.trunc()).integer()));
            return;
        }
        self.real(match operator {
            O::Abs => v.abs(),
            O::Neg => -v,
            O::Floor => v.floor(),
            O::Ceiling => v.ceil(),
            O::Round => (v + 0.5).floor(),
            O::Truncate => v.trunc(),
            O::Sqrt => v.sqrt(),
            O::Ln => v.ln(),
            O::Log => v.log10(),
            O::Sin => v.to_radians().sin(),
            O::Cos => v.to_radians().cos(),
            _ => v,
        });
    }

    fn compare(&mut self, operator: Operator) {
        let b = self.pop();
        let a = self.pop();
        let equal = match (a, b) {
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::Boolean(_), _) | (_, Value::Boolean(_)) => false,
            _ => a.number() == b.number(),
        };
        let (a, b) = (a.number(), b.number());
        self.push(Value::Boolean(match operator {
            Operator::Eq => equal,
            Operator::Ne => !equal,
            Operator::Ge => a >= b,
            Operator::Gt => a > b,
            Operator::Le => a <= b,
            _ => a < b,
        }));
    }
}
