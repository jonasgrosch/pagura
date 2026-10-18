#![forbid(unsafe_code)]
//! Font loading and decoding through the public API, checked against the
//! independently generated vectors of `tools/fixturegen/text.py`.
mod support;

mod descriptor;
mod substitution;
mod tolerance;
mod vectors;
