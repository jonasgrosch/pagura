//! Readers for the oracles that `tools/fixturegen/text.py` derives from the
//! data files independently: PDF arrays of rows in the `text` fixture area.
use pagura_core::{
    Error, Limits, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_testkit::fixture;

/// The rows of the fixture `text/{name}.{suffix}`.
pub(crate) fn rows(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    match Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()? {
        Value::Array(values) => Ok(values),
        _ => Err(Error::syntax("oracle must be an array")),
    }
}

pub(crate) fn row(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("oracle must be a row"))
}

pub(crate) fn text(value: &Value) -> Option<&str> {
    value
        .as_string()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}

/// The case names an inventory oracle lists, one per row.
pub(crate) fn names(inventory: &str) -> Result<Vec<String>> {
    rows(inventory, "out")?
        .iter()
        .map(|value| {
            text(&row(value)?[0])
                .map(str::to_owned)
                .ok_or_else(|| Error::syntax("oracle name must be a string"))
        })
        .collect()
}
