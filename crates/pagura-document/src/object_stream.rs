//! Lazy object-stream decoding and indexed access, clause 7.5.7.

use crate::xref::integer_offset;
use crate::{Document, IndirectObject, Location, ObjectSource};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Diagnostics, Error, ObjectId, Result, Value};
use std::collections::BTreeSet;

#[derive(Debug)]
pub(crate) struct Contents {
    data: Vec<u8>,
    entries: Vec<(u32, usize)>,
}

impl Contents {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
    pub(crate) fn identities(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, &(number, _))| {
                u32::try_from(index).ok().map(|index| (number, index))
            })
    }
}

pub(crate) fn parse(
    document: &Document,
    id: ObjectId,
    stream: u32,
    index: u32,
    path: &mut Vec<ObjectId>,
) -> Result<(IndirectObject, Vec<pagura_core::Diagnostic>)> {
    let stream_id = ObjectId::new(stream, 0);
    let slot = document
        .slots
        .get(&stream_id)
        .ok_or_else(|| Error::missing_object("object stream is missing").in_object(stream_id))?;
    if !matches!(slot.location, Location::Offset(_)) {
        return Err(Error::syntax("object streams cannot be compressed").in_object(stream_id));
    }
    let object = document.object_inner(stream_id, path)?;
    let dictionary = object
        .dictionary()
        .filter(|dictionary| dictionary.is_name(b"Type", b"ObjStm"))
        .ok_or_else(|| {
            Error::syntax("compressed entry must reference an ObjStm").in_object(stream_id)
        })?;
    if dictionary
        .get(b"Extends")
        .is_some_and(|value| !matches!(value, Value::Reference(_)))
    {
        return Err(
            Error::syntax("ObjStm Extends must be an indirect reference").in_object(stream_id),
        );
    }
    let cache = document
        .object_streams
        .get(&stream)
        .ok_or(Error::syntax("missing object stream cache"))?;
    // Resolve dependencies before entering OnceLock: no lock is held while
    // loading another object, including mutually dependent stream parameters.
    let contents = if let Some(contents) = cache.get() {
        contents
    } else {
        path.push(stream_id);
        let prepared = document.prepare_stream(object, path);
        path.pop();
        let (encoded, chain) = prepared?;
        cache.get_or_init(|| {
            let mut diagnostics = Diagnostics::new(document.recovery());
            let decoded = chain
                .decode(&encoded, document.limits().decoded_bytes, &mut diagnostics)
                .map_err(|error| error.in_object(stream_id))?;
            document.record(
                diagnostics
                    .take()
                    .into_iter()
                    .map(|diagnostic| diagnostic.in_object(stream_id))
                    .collect(),
            );
            headers(
                decoded.into_owned(),
                dictionary,
                document.limits(),
                stream_id,
            )
            .map_err(|error| error.in_object(stream_id))
        })
    };
    let contents = contents.as_ref().map_err(Clone::clone)?;
    let index_usize = usize::try_from(index)
        .map_err(|_| Error::syntax("object stream index out of range").in_object(id))?;
    let &(number, start) = contents
        .entries
        .get(index_usize)
        .filter(|&&(number, _)| number == id.number)
        .ok_or_else(|| {
            Error::syntax("compressed entry disagrees with object stream header").in_object(id)
        })?;
    let end = contents
        .entries
        .get(index_usize + 1)
        .map_or(contents.data.len(), |&(_, offset)| offset);
    let bytes = contents
        .data
        .get(..end)
        .ok_or(Error::syntax("object stream extent outside data"))?;
    let mut parser = Parser::at(bytes, start, Origin::Stream(stream_id), document.limits())?
        .with_recovery(document.recovery());
    let value = parser.parse().map_err(|error| error.in_object(id))?;
    if !parser.at_end() {
        return Err(
            Error::syntax("compressed object has trailing tokens or a nested stream")
                .at(parser.position())
                .in_object(id),
        );
    }
    let diagnostics = parser
        .take_diagnostics()
        .into_iter()
        .map(|diagnostic| diagnostic.in_object(id))
        .collect();
    Ok((
        IndirectObject::new(
            ObjectId::new(number, 0),
            value,
            None,
            ObjectSource::Compressed { stream, index },
        ),
        diagnostics,
    ))
}

pub(crate) fn headers(
    data: Vec<u8>,
    dictionary: &pagura_core::Dictionary,
    limits: pagura_core::Limits,
    id: ObjectId,
) -> Result<Contents> {
    let count = integer_offset(dictionary, b"N")?
        .ok_or(Error::syntax("ObjStm N is missing").in_object(id))?;
    let first = integer_offset(dictionary, b"First")?
        .ok_or(Error::syntax("ObjStm First is missing").in_object(id))?;
    // Each pair requires at least two digits and separating white space.
    if count > limits.objects || count > data.len() / 4 {
        return Err(Error::limit("object stream header budget exceeded").in_object(id));
    }
    let header = data
        .get(..first)
        .ok_or(Error::syntax("ObjStm First lies outside decoded data").in_object(id))?;
    let mut parser = Parser::new(header, Origin::Stream(id), limits);
    let mut entries = Vec::with_capacity(count);
    let mut numbers = BTreeSet::new();
    for _ in 0..count {
        let number = u32::try_from(parser.unsigned()?)
            .ok()
            .filter(|&number| number != 0)
            .ok_or(
                Error::syntax("invalid compressed object number")
                    .at(parser.position())
                    .in_object(id),
            )?;
        let offset = usize::try_from(parser.unsigned()?)
            .ok()
            .and_then(|offset| first.checked_add(offset))
            .filter(|&offset| offset < data.len())
            .ok_or(
                Error::syntax("compressed object offset outside stream")
                    .at(parser.position())
                    .in_object(id),
            )?;
        if !numbers.insert(number)
            || entries
                .last()
                .is_some_and(|&(_, previous)| previous >= offset)
        {
            return Err(
                Error::syntax("duplicate object or unordered offsets in ObjStm")
                    .at(parser.position())
                    .in_object(id),
            );
        }
        entries.push((number, offset));
    }
    if !parser.at_end() {
        return Err(Error::syntax("ObjStm header exceeds N")
            .at(parser.position())
            .in_object(id));
    }
    if entries.first().is_some_and(|&(_, offset)| offset != first) {
        return Err(Error::syntax("first ObjStm object offset must be zero").in_object(id));
    }
    Ok(Contents { data, entries })
}
