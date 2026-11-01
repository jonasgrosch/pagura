//! Static validation of calculator programs (ADR 0034): an abstract run
//! over operand kinds that proves every run has enough operands of the right
//! kind, stays within the operand stack and ends with the result count, and
//! counts the most work a run can do.
//!
//! Integer constants are tracked through integer arithmetic so that the
//! counts of `copy`, `index` and `roll` are known; a count that is not known
//! makes the stack effect depend on the data, which is not validated.
//! Boolean constants are tracked too: a conditional whose condition is
//! known runs, and is validated, only on its taken branch.

use super::{Node, Operator, Value};
use pagura_core::{Error, Limits, Position, Result};

mod branches;

/// What validation knows about an operand.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// A boolean, with its value when it is the same in every run.
    Boolean(Option<bool>),
    /// An integer, with its value when it is the same in every run.
    Integer(Option<i32>),
    Real,
    /// A real in this closed finite interval, only in the fallback proof.
    Bounded([f64; 2]),
    /// An integer or a real: integer arithmetic whose result may have
    /// overflowed into a real.
    Number,
}

impl Kind {
    fn of(value: Value) -> Self {
        match value {
            Value::Integer(n) => Self::Integer(Some(n)),
            Value::Real(_) => Self::Real,
            Value::Boolean(b) => Self::Boolean(Some(b)),
        }
    }

    /// The kind both branches of a conditional may leave, if they agree on
    /// booleans and numbers.
    fn join(self, other: Self) -> Option<Self> {
        Some(match (self, other) {
            (Self::Boolean(a), Self::Boolean(b)) => Self::Boolean(a.filter(|_| a == b)),
            (Self::Boolean(_), _) | (_, Self::Boolean(_)) => return None,
            (Self::Integer(a), Self::Integer(b)) => Self::Integer(a.filter(|_| a == b)),
            (Self::Real | Self::Bounded(_), Self::Real) | (Self::Real, Self::Bounded(_)) => {
                Self::Real
            }
            (Self::Bounded(a), Self::Bounded(b)) => Self::Bounded([a[0].min(b[0]), a[1].max(b[1])]),
            _ => Self::Number,
        })
    }

    fn numeric(self) -> Result<Self> {
        if matches!(self, Self::Boolean(_)) {
            Err(Error::syntax("calculator requires numeric operand"))
        } else {
            Ok(self)
        }
    }

    /// The operand as an integer: its constant when known.
    fn integer(self) -> Result<Option<i32>> {
        match self {
            Self::Integer(n) => Ok(n),
            Self::Number => Ok(None),
            Self::Real | Self::Bounded(_) | Self::Boolean(_) => {
                Err(Error::syntax("calculator requires integer"))
            }
        }
    }

    /// The operand as a `copy`, `index` or `roll` count, which must be known.
    fn count(self) -> Result<i32> {
        self.integer()?.ok_or(Error::unsupported(
            crate::features::CALCULATOR,
            "calculator stack effect depends on its data",
        ))
    }
}

/// Preserves the existing proof, cost and failures. Only an unsupported
/// calculator may additionally qualify through the bounded Domain proof.
pub(super) fn with_domain(program: &[Node], domain: &[[f64; 2]], outputs: usize) -> Result<usize> {
    match check(program, domain.len(), outputs) {
        Err(error)
            if error.kind == pagura_core::ErrorKind::Unsupported(crate::features::CALCULATOR)
                && error.feature() == Some(crate::features::CALCULATOR) =>
        {
            // A failed extra proof never replaces the source's original
            // structured refusal, including its exact position and feature.
            branches::check(program, domain, outputs).or(Err(error))
        }
        result => result,
    }
}

/// Validates `program` run on `inputs` real operands and producing
/// `outputs` numbers, and returns the most work one run costs.
pub(super) fn check(program: &[Node], inputs: usize, outputs: usize) -> Result<usize> {
    let mut stack = Stack {
        kinds: vec![Kind::Real; inputs],
    };
    if inputs > Limits::CALCULATOR_STACK {
        return Err(Error::limit("calculator stack exceeded"));
    }
    let cost = stack.run(program)?;
    if stack.kinds.len() != outputs {
        return Err(Error::syntax("wrong calculator output count"));
    }
    for kind in &stack.kinds {
        kind.numeric()?;
    }
    Ok(cost)
}

/// The operand kinds on the stack at one point of every run.
#[derive(Debug, Clone)]
struct Stack {
    kinds: Vec<Kind>,
}

impl Stack {
    /// Runs `program` abstractly, returning its most work.
    fn run(&mut self, program: &[Node]) -> Result<usize> {
        let mut cost = 0_usize;
        for node in program {
            let work = match node {
                Node::Push(value, offset) => {
                    self.push(Kind::of(*value)).map_err(|e| at(e, *offset))?;
                    1
                }
                Node::Operator(operator, offset) => self
                    .operator(*operator)
                    .map_err(|e| at(e, *offset))?
                    .saturating_add(1),
                Node::Conditional {
                    then,
                    otherwise,
                    offset,
                } => self
                    .conditional(then, otherwise.as_deref())
                    .map_err(|e| at(e, *offset))?
                    .saturating_add(1),
            };
            cost = cost.saturating_add(work);
        }
        Ok(cost)
    }

    /// Validates the branches that can run and joins what they leave.
    fn conditional(&mut self, then: &[Node], otherwise: Option<&[Node]>) -> Result<usize> {
        let Kind::Boolean(condition) = self.pop()? else {
            return Err(Error::syntax("calculator condition must be boolean"));
        };
        match condition {
            Some(true) => return self.run(then),
            Some(false) => return otherwise.map_or(Ok(0), |nodes| self.run(nodes)),
            None => {}
        }
        let mut other = self.clone();
        let first = self.run(then)?;
        let second = otherwise
            .map(|nodes| other.run(nodes))
            .transpose()?
            .unwrap_or(0);
        if self.kinds.len() != other.kinds.len() {
            return Err(Error::unsupported(
                crate::features::CALCULATOR,
                "calculator branches leave different stack depths",
            ));
        }
        for (kind, other) in self.kinds.iter_mut().zip(&other.kinds) {
            *kind = kind.join(*other).ok_or(Error::unsupported(
                crate::features::CALCULATOR,
                "calculator branches leave different operand kinds",
            ))?;
        }
        Ok(first.max(second))
    }

    fn push(&mut self, kind: Kind) -> Result<()> {
        if self.kinds.len() >= Limits::CALCULATOR_STACK {
            return Err(Error::limit("calculator stack exceeded"));
        }
        self.kinds.push(kind);
        Ok(())
    }

    fn pop(&mut self) -> Result<Kind> {
        self.kinds
            .pop()
            .ok_or(Error::syntax("calculator stack underflow"))
    }

    /// Applies `operator`, returning the work it adds beyond its own unit.
    fn operator(&mut self, operator: Operator) -> Result<usize> {
        use Operator as O;
        match operator {
            O::Pop | O::Dup | O::Exch | O::Copy | O::Index | O::Roll => {
                return self.stack_operator(operator);
            }
            O::Abs | O::Neg => {
                let kind = self.pop()?.numeric()?;
                self.push(match kind {
                    Kind::Integer(Some(n)) => {
                        let result = if operator == O::Abs {
                            n.checked_abs()
                        } else {
                            n.checked_neg()
                        };
                        result.map_or(Kind::Real, |n| Kind::Integer(Some(n)))
                    }
                    Kind::Integer(None) => Kind::Number,
                    Kind::Bounded(_) => Kind::Real,
                    other => other,
                })?;
            }
            O::Floor | O::Ceiling | O::Round | O::Truncate => {
                let kind = self.pop()?.numeric()?;
                self.push(match kind {
                    Kind::Bounded(_) => Kind::Real,
                    other => other,
                })?;
            }
            O::Cvi => {
                let kind = self.pop()?.numeric()?;
                self.push(match kind {
                    Kind::Integer(n) => Kind::Integer(n),
                    _ => Kind::Integer(None),
                })?;
            }
            O::Cvr | O::Sqrt | O::Ln | O::Log | O::Sin | O::Cos => {
                self.pop()?.numeric()?;
                self.push(Kind::Real)?;
            }
            O::Add | O::Sub | O::Mul => self.arithmetic(operator)?,
            O::Div | O::Exp | O::Atan => {
                self.pop()?.numeric()?;
                self.pop()?.numeric()?;
                self.push(Kind::Real)?;
            }
            O::Idiv | O::Mod | O::Bitshift => {
                let b = self.pop()?.integer()?;
                let a = self.pop()?.integer()?;
                let value = a.zip(b).map(|(a, b)| super::run::integer(operator, a, b));
                self.push(Kind::Integer(value))?;
            }
            O::Not => {
                let kind = match self.pop()? {
                    Kind::Boolean(b) => Kind::Boolean(b.map(|b| !b)),
                    Kind::Integer(n) => Kind::Integer(n.map(|n| !n)),
                    Kind::Number => Kind::Integer(None),
                    Kind::Real | Kind::Bounded(_) => {
                        return Err(Error::syntax("invalid calculator logical operand"));
                    }
                };
                self.push(kind)?;
            }
            O::And | O::Or | O::Xor => self.logical(operator)?,
            O::Eq | O::Ne => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = match (a, b) {
                    (Kind::Boolean(Some(a)), Kind::Boolean(Some(b))) => Some(a == b),
                    (Kind::Integer(Some(a)), Kind::Integer(Some(b))) => Some(a == b),
                    _ => None,
                };
                self.push(Kind::Boolean(equal.map(|e| e == (operator == O::Eq))))?;
            }
            O::Ge | O::Gt | O::Le | O::Lt => {
                let b = self.pop()?;
                let a = self.pop()?.numeric()?;
                b.numeric()?;
                let value = match (a, b) {
                    (Kind::Integer(Some(a)), Kind::Integer(Some(b))) => Some(match operator {
                        O::Ge => a >= b,
                        O::Gt => a > b,
                        O::Le => a <= b,
                        _ => a < b,
                    }),
                    _ => comparison(operator, a, b),
                };
                self.push(Kind::Boolean(value))?;
            }
        }
        Ok(0)
    }

    /// `add`, `sub` and `mul`: integers stay integers unless they overflow.
    fn arithmetic(&mut self, operator: Operator) -> Result<()> {
        let b = self.pop()?.numeric()?;
        let a = self.pop()?.numeric()?;
        self.push(match (a, b) {
            (Kind::Integer(Some(a)), Kind::Integer(Some(b))) => {
                super::run::checked(operator, a, b).map_or(Kind::Real, |n| Kind::Integer(Some(n)))
            }
            (Kind::Real | Kind::Bounded(_), _) | (_, Kind::Real | Kind::Bounded(_)) => Kind::Real,
            _ => Kind::Number,
        })
    }

    /// `and`, `or` and `xor` on two booleans or two integers.
    fn logical(&mut self, operator: Operator) -> Result<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let kind = match (a, b) {
            (Kind::Boolean(a), Kind::Boolean(b)) => {
                Kind::Boolean(a.zip(b).map(|(a, b)| match operator {
                    Operator::And => a & b,
                    Operator::Or => a | b,
                    _ => a ^ b,
                }))
            }
            (Kind::Integer(_) | Kind::Number, Kind::Integer(_) | Kind::Number) => {
                let value = a.integer()?.zip(b.integer()?);
                Kind::Integer(value.map(|(a, b)| super::run::integer(operator, a, b)))
            }
            _ => return Err(Error::syntax("mismatched calculator logical operands")),
        };
        self.push(kind)
    }

    /// The stack operators, returning the values `copy` and `roll` move.
    fn stack_operator(&mut self, operator: Operator) -> Result<usize> {
        let underflow = |message| Error::syntax(message);
        match operator {
            Operator::Pop => {
                self.pop()?;
            }
            Operator::Dup => {
                let top = *self
                    .kinds
                    .last()
                    .ok_or(Error::syntax("calculator stack underflow"))?;
                self.push(top)?;
            }
            Operator::Exch => {
                let a = self.pop()?;
                let b = self.pop()?;
                self.push(a)?;
                self.push(b)?;
            }
            Operator::Copy => {
                let count = usize::try_from(self.pop()?.count()?)
                    .map_err(|_| Error::syntax("negative calculator copy"))?;
                let start = self
                    .kinds
                    .len()
                    .checked_sub(count)
                    .ok_or(underflow("calculator copy underflow"))?;
                for i in start..start + count {
                    let kind = self.kinds.get(i).copied().unwrap_or(Kind::Real);
                    self.push(kind)?;
                }
                return Ok(count);
            }
            Operator::Index => {
                let n = usize::try_from(self.pop()?.count()?)
                    .map_err(|_| Error::syntax("negative calculator index"))?;
                let index = self
                    .kinds
                    .len()
                    .checked_sub(n)
                    .and_then(|v| v.checked_sub(1))
                    .ok_or(underflow("calculator index underflow"))?;
                let kind = self.kinds.get(index).copied().unwrap_or(Kind::Real);
                self.push(kind)?;
            }
            Operator::Roll => return self.roll(),
            _ => return Err(Error::syntax("unknown calculator stack operator")),
        }
        Ok(0)
    }

    /// `n j roll`: `n` must be known; an unknown shift joins the kinds of
    /// the rolled operands.
    fn roll(&mut self) -> Result<usize> {
        let shift = self.pop()?.integer()?;
        let n = self.pop()?.count()?;
        let count = usize::try_from(n).map_err(|_| Error::syntax("negative calculator roll"))?;
        let start = self
            .kinds
            .len()
            .checked_sub(count)
            .ok_or(Error::syntax("calculator roll underflow"))?;
        let rolled = self.kinds.get_mut(start..).unwrap_or_default();
        if let Some(shift) = shift {
            if n > 0 {
                let shift = usize::try_from(shift.rem_euclid(n)).unwrap_or(0);
                rolled.rotate_right(shift);
            }
        } else if let Some((&first, rest)) = rolled.split_first() {
            let joined = rest.iter().try_fold(first, |kind, &next| {
                kind.join(next).ok_or(Error::unsupported(
                    crate::features::CALCULATOR,
                    "calculator roll of mixed operands depends on its data",
                ))
            })?;
            rolled.fill(joined);
        }
        Ok(count)
    }
}

/// Interval separation proves a comparison only for finite real operands
/// and exactly representable integer constants. No input correlation is
/// inferred; overlapping intervals leave the condition unknown.
fn comparison(operator: Operator, a: Kind, b: Kind) -> Option<bool> {
    let bounds = |kind| match kind {
        Kind::Bounded(range) => Some(range),
        Kind::Integer(Some(n)) => Some([f64::from(n); 2]),
        _ => None,
    };
    let ([low_a, high_a], [low_b, high_b]) = (bounds(a)?, bounds(b)?);
    let (certain, impossible) = match operator {
        Operator::Ge => (low_a >= high_b, high_a < low_b),
        Operator::Gt => (low_a > high_b, high_a <= low_b),
        Operator::Le => (high_a <= low_b, low_a > high_b),
        Operator::Lt => (high_a < low_b, low_a >= high_b),
        _ => return None,
    };
    if certain {
        Some(true)
    } else if impossible {
        Some(false)
    } else {
        None
    }
}

/// Places a validation error at its instruction, unless it has a position.
fn at(error: Error, offset: usize) -> Error {
    if error.position == Position::Unknown {
        error.at(Position::Buffer(offset))
    } else {
        error
    }
}
