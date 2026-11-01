//! A conservative fallback with at most two joined operand-stack depths.
//!
//! Each syntax subtree runs once with its whole state set, so each operator
//! is applied at most twice. The previous checker plus this proof therefore
//! visits at most three abstract operands per syntax instruction. Stack copies
//! and joins are bounded by the existing 100-entry architectural stack.
//! Neither evaluation nor its arithmetic changes (7.10.5.2, 7.10.5.3).

use super::{Kind, Node, Result, Stack, Value, at};
use pagura_core::Error;

/// An operand stack and the greatest executed work reaching it.
#[derive(Debug, Clone)]
struct State {
    stack: Stack,
    cost: usize,
}

/// A fixed two-slot proof frontier, joined by stack depth.
#[derive(Debug, Default)]
struct States {
    slots: [Option<State>; 2],
}

pub(super) fn check(program: &[Node], domain: &[[f64; 2]], outputs: usize) -> Result<usize> {
    let mut states = States::default();
    states.insert(State {
        stack: Stack {
            kinds: domain.iter().copied().map(Kind::Bounded).collect(),
        },
        cost: 0,
    })?;
    states.run(program)?;
    let mut cost = 0;
    for state in states.slots.into_iter().flatten() {
        if state.stack.kinds.len() != outputs {
            return Err(Error::syntax("wrong calculator output count"));
        }
        for kind in state.stack.kinds {
            kind.numeric()?;
        }
        cost = cost.max(state.cost);
    }
    Ok(cost)
}

impl States {
    /// Equal depths join their operand facts and maximum prefix work.
    /// A third depth, or unjoinable kinds, remains explicitly unsupported.
    fn insert(&mut self, state: State) -> Result<()> {
        for previous in self.slots.iter_mut().flatten() {
            if previous.stack.kinds.len() == state.stack.kinds.len() {
                for (kind, other) in previous.stack.kinds.iter_mut().zip(state.stack.kinds) {
                    *kind = kind.join(other).ok_or_else(unsupported)?;
                }
                previous.cost = previous.cost.max(state.cost);
                return Ok(());
            }
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or_else(unsupported)?;
        *slot = Some(state);
        Ok(())
    }

    fn run(&mut self, program: &[Node]) -> Result<()> {
        for node in program {
            match node {
                Node::Push(value, offset) => {
                    let kind = match value {
                        Value::Real(number) => Kind::Bounded([*number; 2]),
                        value => Kind::of(*value),
                    };
                    for state in self.slots.iter_mut().flatten() {
                        state.stack.push(kind).map_err(|error| at(error, *offset))?;
                        state.cost = state.cost.saturating_add(1);
                    }
                }
                Node::Operator(operator, offset) => {
                    for state in self.slots.iter_mut().flatten() {
                        let work = state
                            .stack
                            .operator(*operator)
                            .map_err(|error| at(error, *offset))?;
                        state.cost = state.cost.saturating_add(work).saturating_add(1);
                    }
                }
                Node::Conditional {
                    then,
                    otherwise,
                    offset,
                } => {
                    self.conditional(then, otherwise.as_deref())
                        .map_err(|error| at(error, *offset))?;
                }
            }
        }
        Ok(())
    }

    fn conditional(&mut self, then: &[Node], otherwise: Option<&[Node]>) -> Result<()> {
        let mut taken = Self::default();
        let mut skipped = Self::default();
        for mut state in std::mem::take(self).slots.into_iter().flatten() {
            let Kind::Boolean(condition) = state.stack.pop()? else {
                return Err(Error::syntax("calculator condition must be boolean"));
            };
            state.cost = state.cost.saturating_add(1);
            if condition != Some(false) {
                taken.insert(state.clone())?;
            }
            if condition != Some(true) {
                skipped.insert(state)?;
            }
        }
        taken.run(then)?;
        if let Some(otherwise) = otherwise {
            skipped.run(otherwise)?;
        }
        *self = taken;
        for state in skipped.slots.into_iter().flatten() {
            self.insert(state)?;
        }
        Ok(())
    }
}

fn unsupported() -> Error {
    Error::unsupported(
        crate::features::CALCULATOR,
        "calculator branch alternatives cannot be joined",
    )
}
