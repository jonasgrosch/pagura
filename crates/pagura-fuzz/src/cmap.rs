//! `cmap`: a predefined CMap program, as a font provider supplies one,
//! parses within its budgets or fails with a structured error, and decoding
//! consumes one to four bytes per code (9.7.5, 9.7.6.2;
//! `pagura::provider::PredefinedCMap`).
//!
//! Layout: parts split at [`super::wrapper::SEPARATOR`]: the CMap program,
//! the program that any `usecmap` names, and the bytes to decode.

use pagura::Error;
use pagura::provider::PredefinedCMap;
use std::sync::Arc;

/// How many codes of the input are decoded.
const CODES: usize = 256;

/// Parses the program, with the second part as its parent, and decodes the
/// third part code by code.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let parts = super::wrapper::parts(data);
    let (program, parent, codes) = match parts.as_slice() {
        [program] => (*program, None, &[][..]),
        [program, parent] => (*program, Some(*parent), &[][..]),
        [program, parent, codes, ..] => (*program, Some(*parent), *codes),
        [] => return,
    };
    let limits = super::limits();
    let resolve = |_: &[u8]| {
        let parent = parent.ok_or_else(|| Error::missing_object("no parent CMap supplied"))?;
        let nested = |_: &[u8]| Err(Error::missing_object("only one parent CMap supplied"));
        PredefinedCMap::parse(parent, &limits, nested).map(Arc::new)
    };
    let Some(cmap) = super::checked(PredefinedCMap::parse(program, &limits, resolve)) else {
        return;
    };
    let _ = (cmap.writing_mode(), cmap.collection());
    let mut rest = codes;
    for _ in 0..CODES {
        if rest.is_empty() {
            break;
        }
        let Some((_, length, _)) = super::checked(cmap.decode_code(rest, &limits)) else {
            break;
        };
        let length = usize::from(length);
        assert!(
            (1..=4).contains(&length) && length <= rest.len(),
            "a {length}-byte code from {} bytes",
            rest.len()
        );
        rest = &rest[length..];
    }
}
