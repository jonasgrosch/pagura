//! One syntax validator for new documents and incremental revisions.

use crate::NewObject;
use pagura_core::{Dictionary, Error, Limits, Meter, Name, ObjectId, Result, Value};

/// Validates an object and its references before serialization or staging.
///
/// `contains` defines the caller's generation-aware reference namespace.
/// Stored stream bytes must already agree with the declared filters.
///
/// # Errors
/// Returns `InvalidArgument` for unrepresentable values, `MissingObject` for
/// rejected references, and `Limit` for syntax or stream budget exhaustion.
pub fn object(
    object: &NewObject,
    limits: &Limits,
    contains: &impl Fn(ObjectId) -> bool,
) -> Result<()> {
    let mut nodes = Meter::new(limits.value_nodes, "writer value budget exceeded");
    match object {
        NewObject::Value(value) => value_tree(value, 0, limits, &mut nodes, contains),
        NewObject::Stream { dictionary, data } => {
            if data.len() > limits.stream_bytes {
                return Err(Error::limit("writer stream budget exceeded"));
            }
            name(&Name::new(crate::writer::STREAM_LENGTH), limits)?;
            // The dictionary and the automatically written Length value.
            nodes.charge(2)?;
            entries(dictionary, true, 1, limits, &mut nodes, contains)
        }
    }
}

fn value_tree(
    value: &Value,
    depth: usize,
    limits: &Limits,
    nodes: &mut Meter,
    contains: &impl Fn(ObjectId) -> bool,
) -> Result<()> {
    if depth > limits.depth {
        return Err(Error::limit("writer nesting depth exceeded"));
    }
    nodes.charge(1)?;
    match value {
        Value::Real(value) if !value.is_finite() => {
            Err(Error::invalid_argument("PDF reals must be finite"))
        }
        Value::String(bytes) if bytes.len() > limits.string_bytes => {
            Err(Error::limit("writer string budget exceeded"))
        }
        Value::Name(value) => name(value, limits),
        Value::Reference(id) if !contains(*id) => {
            Err(Error::missing_object("dangling object reference").in_object(*id))
        }
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| value_tree(value, depth + 1, limits, nodes, contains)),
        Value::Dictionary(dictionary) => {
            entries(dictionary, false, depth + 1, limits, nodes, contains)
        }
        _ => Ok(()),
    }
}

fn entries(
    dictionary: &Dictionary,
    stream: bool,
    depth: usize,
    limits: &Limits,
    nodes: &mut Meter,
    contains: &impl Fn(ObjectId) -> bool,
) -> Result<()> {
    // Length itself is a value at this depth, even in an empty stream dictionary.
    if stream && depth > limits.depth {
        return Err(Error::limit("writer nesting depth exceeded"));
    }
    for (key, value) in dictionary {
        if stream && key.as_bytes() == b"Length" {
            continue;
        }
        name(key, limits)?;
        value_tree(value, depth, limits, nodes, contains)?;
    }
    Ok(())
}

fn name(name: &Name, limits: &Limits) -> Result<()> {
    if name.len() > limits.string_bytes {
        return Err(Error::limit("writer name budget exceeded"));
    }
    if name.as_bytes().contains(&0) {
        return Err(Error::invalid_argument("PDF names cannot contain NUL"));
    }
    Ok(())
}
