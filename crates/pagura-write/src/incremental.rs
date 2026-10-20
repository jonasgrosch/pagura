//! Append-only revisions (ISO 32000-2, 7.5.6 and 14.4).

use crate::{NewObject, validation, writer::Writer};
use pagura_core::{Dictionary, Error, Limits, Name, ObjectId, Result, ResultExt, Value};
use pagura_crypto::sha256;
use std::collections::BTreeMap;

/// Cross-reference representation for the appended revision (7.5.4, 7.5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefFormat {
    /// Sparse classic table; suitable for classic-table source files.
    Table,
    /// Unencoded xref stream; retains modern-reader compatibility for stream
    /// and hybrid sources, including unchanged compressed objects.
    Stream,
}

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
    format: XrefFormat,
    limits: &Limits,
) -> Result<Vec<u8>> {
    let mut writer = Writer::new(limits.file_bytes);
    if objects.is_empty() {
        writer.put(original)?;
        return Ok(writer.into_bytes());
    }
    let (mut trailer, xref_id, count) =
        prepare(original, previous_xref, trailer, objects, format, limits)?;
    let permanent = permanent_id(&trailer, original)?;
    writer.put(original)?;
    writer.put(b"\n")?;
    let mut offsets = Vec::new();
    offsets
        .try_reserve_exact(count)
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
    match format {
        XrefFormat::Table => {
            writer.put(b"xref\n")?;
            for (id, offset) in offsets {
                writer.text(format_args!(
                    "{} 1\n{offset:010} {:05} n \n",
                    id.number, id.generation
                ))?;
            }
            writer.put(b"trailer\n")?;
            writer.dictionary(&trailer, None)?;
        }
        XrefFormat::Stream => {
            offsets.push((xref_id, xref));
            write_xref_stream(&mut writer, xref_id, trailer, &offsets, limits)?;
        }
    }
    writer.text(format_args!("\nstartxref\n{xref}\n%%EOF\n"))?;
    Ok(writer.into_bytes())
}

fn prepare(
    original: &[u8],
    previous_xref: usize,
    trailer: &Dictionary,
    objects: &BTreeMap<ObjectId, NewObject>,
    format: XrefFormat,
    limits: &Limits,
) -> Result<(Dictionary, ObjectId, usize)> {
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
    let count = objects
        .len()
        .checked_add(usize::from(format == XrefFormat::Stream))
        .ok_or(Error::limit("incremental xref count overflow"))?;
    if count > limits.objects
        || count
            .checked_mul(size_of::<(ObjectId, usize)>())
            .is_none_or(|bytes| bytes > limits.document_bytes)
    {
        return Err(Error::limit("incremental xref storage budget exceeded"));
    }
    let size = validate_objects(objects, size, limits)?;
    let xref_id = ObjectId::new(size, 0);
    let size = if format == XrefFormat::Stream {
        size.checked_add(1)
            .ok_or(Error::limit("xref stream object number range exhausted"))?
    } else {
        size
    };
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
    Ok((trailer, xref_id, count))
}

fn write_xref_stream(
    writer: &mut Writer,
    id: ObjectId,
    mut dictionary: Dictionary,
    offsets: &[(ObjectId, usize)],
    limits: &Limits,
) -> Result<()> {
    // Type (1 byte), absolute offset (8), generation (2), all big endian.
    let length = offsets
        .len()
        .checked_mul(11)
        .filter(|length| *length <= limits.stream_bytes && *length <= limits.document_bytes)
        .ok_or(Error::limit("incremental xref stream budget exceeded"))?;
    let mut data = Vec::new();
    data.try_reserve_exact(length)
        .map_err(|_| Error::limit("incremental xref stream allocation failed"))?;
    let count = offsets
        .len()
        .checked_mul(2)
        .filter(|count| {
            *count <= limits.value_nodes
                && count.saturating_mul(size_of::<Value>()) <= limits.document_bytes
        })
        .ok_or(Error::limit("incremental xref index budget exceeded"))?;
    let mut index = Vec::new();
    index
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("incremental xref index allocation failed"))?;
    for &(object, offset) in offsets {
        data.push(1);
        data.extend_from_slice(
            &u64::try_from(offset)
                .map_err(|_| Error::limit("xref offset exceeds stream range"))?
                .to_be_bytes(),
        );
        data.extend_from_slice(&object.generation.to_be_bytes());
        index.extend([Value::Integer(i64::from(object.number)), Value::Integer(1)]);
    }
    dictionary.insert(Name::new(b"Type"), Value::name(b"XRef"));
    dictionary.insert(
        Name::new(b"W"),
        Value::Array(vec![
            Value::Integer(1),
            Value::Integer(8),
            Value::Integer(2),
        ]),
    );
    dictionary.insert(Name::new(b"Index"), Value::Array(index));
    let object = NewObject::Stream { dictionary, data };
    validation::object(&object, limits, &|_| true)?;
    writer.object(id, &object)
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
