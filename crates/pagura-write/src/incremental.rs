//! Append-only revisions (ISO 32000-2, 7.5.6 and 14.4).

use crate::{NewObject, validation, writer::Writer};
use pagura_core::{Dictionary, Error, Limits, Name, ObjectId, Result, ResultExt, Value};
use pagura_crypto::sha256;
use std::collections::BTreeMap;

/// Serializes replacements as a new revision after the exact original bytes.
///
/// The caller supplies a verified previous xref offset and effective trailer,
/// and must check identities, existing references, signatures and source
/// recovery before calling. This syntax layer does not parse the original.
/// References to original objects are allowed. Objects are ordered by number;
/// two generations of one number are rejected. No objects are freed.
/// An empty update returns the original bytes exactly. Unknown trailer entries
/// survive; xref-stream transport entries do not become trailer entries.
///
/// # Errors
/// Returns `InvalidArgument` for invalid metadata or encrypted input, `Limit`
/// for syntax/output bounds, and the errors of [`crate::validate_object`].
pub fn write_incremental(
    original: &[u8],
    previous_xref: usize,
    trailer: &Dictionary,
    objects: &BTreeMap<ObjectId, NewObject>,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let mut writer = Writer::new(limits.file_bytes);
    if objects.is_empty() {
        writer.put(original)?;
        return Ok(writer.into_bytes());
    }
    if previous_xref >= original.len()
        || trailer.get(b"Root").and_then(Value::as_reference).is_none()
        || trailer
            .get(b"Encrypt")
            .is_some_and(|value| !value.is_null())
    {
        return Err(Error::invalid_argument(
            "invalid incremental source metadata",
        ));
    }
    let size = trailer
        .get(b"Size")
        .and_then(Value::as_integer)
        .and_then(|size| u32::try_from(size).ok())
        .filter(|size| *size > 0)
        .ok_or(Error::invalid_argument("incremental trailer requires Size"))?;
    let size = validate_objects(objects, size, limits)?;
    if trailer.retained_bytes() > limits.document_bytes {
        return Err(Error::limit("incremental trailer storage budget exceeded"));
    }
    let mut trailer = trailer.clone();
    if trailer.is_name(b"Type", b"XRef") {
        for key in [
            b"Type".as_slice(),
            b"W",
            b"Index",
            b"Length",
            b"Filter",
            b"DecodeParms",
        ] {
            trailer.remove(key);
        }
    }
    trailer.remove(b"XRefStm");
    trailer.insert(Name::new(b"Size"), Value::Integer(i64::from(size)));
    trailer.insert(
        Name::new(b"Prev"),
        Value::Integer(
            i64::try_from(previous_xref)
                .map_err(|_| Error::limit("xref offset exceeds integer range"))?,
        ),
    );
    let permanent = permanent_id(&trailer, original)?;
    writer.put(original)?;
    writer.put(b"\n")?;
    let mut offsets = Vec::new();
    offsets
        .try_reserve_exact(objects.len())
        .map_err(|_| Error::limit("incremental xref storage budget exceeded"))?;
    for (&id, object) in objects {
        offsets.push((id, writer.offset()));
        writer.object(id, object).in_object(id)?;
    }
    // Bind the changing identifier to the original plus canonical new objects;
    // the trailer is deliberately outside its own hash (14.4).
    let version = sha256(writer.bytes()).to_vec();
    trailer.insert(
        Name::new(b"ID"),
        Value::Array(vec![Value::String(permanent), Value::String(version)]),
    );
    validation::object(
        &NewObject::Value(Value::Dictionary(trailer.clone())),
        limits,
        &|_| true,
    )?;
    let xref = writer.offset();
    writer.put(b"xref\n")?;
    for (id, offset) in offsets {
        writer.text(format_args!(
            "{} 1\n{offset:010} {:05} n \n",
            id.number, id.generation
        ))?;
    }
    writer.put(b"trailer\n")?;
    writer.dictionary(&trailer, None)?;
    writer.text(format_args!("\nstartxref\n{xref}\n%%EOF\n"))?;
    Ok(writer.into_bytes())
}

fn validate_objects(
    objects: &BTreeMap<ObjectId, NewObject>,
    mut size: u32,
    limits: &Limits,
) -> Result<u32> {
    if objects.len() > limits.objects
        || objects
            .len()
            .checked_mul(size_of::<(ObjectId, usize)>())
            .is_none_or(|bytes| bytes > limits.document_bytes)
    {
        return Err(Error::limit("incremental xref storage budget exceeded"));
    }
    let mut previous = None;
    for (&id, object) in objects {
        if id.number == 0 || id.generation == u16::MAX || previous == Some(id.number) {
            return Err(
                Error::invalid_argument("invalid incremental object identity").in_object(id),
            );
        }
        previous = Some(id.number);
        size = size.max(
            id.number
                .checked_add(1)
                .ok_or(Error::limit("object number range exhausted"))?,
        );
        validation::object(object, limits, &|_| true).in_object(id)?;
    }
    Ok(size)
}

fn permanent_id(trailer: &Dictionary, original: &[u8]) -> Result<Vec<u8>> {
    match trailer.get(b"ID") {
        None | Some(Value::Null) => Ok(sha256(original).to_vec()),
        Some(Value::Array(ids)) => match ids.as_slice() {
            [Value::String(first), Value::String(_)] if !first.is_empty() => Ok(first.clone()),
            _ => Err(Error::invalid_argument(
                "file ID must contain two byte strings",
            )),
        },
        _ => Err(Error::invalid_argument(
            "file ID must contain two byte strings",
        )),
    }
}
