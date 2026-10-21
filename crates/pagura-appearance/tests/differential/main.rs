#![forbid(unsafe_code)]
//! Differential tests against the appearance code of `pagura-render`, the
//! behavioural reference this crate replaces: every generated fixture that
//! has a form or annotations, compared output for output and error for error.

mod annotations;
mod forms;
mod support;
