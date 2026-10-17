//! Reported reconstruction after a structural cross-reference failure (ADR 0003).
//!
//! The scanner advances monotonically and skips strings and stream payloads.
//! Heuristics follow the ADR 0003 repair catalogue and the damage classes seen
//! in the real-world corpus.

use crate::object::header;
use crate::tags;
use crate::xref::{Location, XrefEntry, XrefTable};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, Name, ObjectId, Position, Result, Value,
};

#[derive(Debug, Default)]
struct Scan {
    table: XrefTable,
    objects: usize,
    trailers: usize,
}

pub(crate) fn read(
    bytes: &[u8],
    limits: Limits,
    diagnostics: &mut Diagnostics,
    position: Position,
) -> Result<XrefTable> {
    diagnostics.repair(
        Diagnostic::new(
            tags::XREF_REBUILT,
            "damaged cross-reference data reconstructed from indirect object definitions",
        )
        .at(position),
    )?;
    let mut scan = Scan::default();
    let mut parser = Parser::new(bytes, Origin::File, limits).with_recovery(diagnostics.mode());
    while !parser.at_end() {
        let offset = parser.offset();
        let result = if parser.keyword(b"trailer") {
            parser.parse().and_then(|value| match value {
                Value::Dictionary(dictionary) => remember(&mut scan, dictionary, limits),
                _ => Err(Error::syntax("trailer must be a dictionary").at(parser.position())),
            })
        } else if let Some(id) = header(bytes, offset, limits) {
            object(&mut scan, &mut parser, id, limits, diagnostics)
        } else {
            match parser.peek() {
                Some(b'(' | b'<' | b'[' | b'/') => parser.parse().map(|_| ()),
                _ => parser.word().map(|_| ()),
            }
        };
        if let Err(error) = result
            && !crate::xref::structural(&error)
        {
            return Err(error);
        }
        // Parsing errors can leave the cursor at the offending token. Never
        // restart a failed parse at an earlier byte, including truncated tails.
        if parser.offset() <= offset {
            parser.seek(offset + 1)?;
        }
        diagnostics.extend(parser.take_diagnostics());
    }
    if scan.table.entries.is_empty() {
        return Err(
            Error::syntax("reconstruction found no complete indirect objects").at(position),
        );
    }
    scan.table.rebuild_remaining = Some(limits.objects.saturating_sub(scan.objects));
    Ok(scan.table)
}

fn remember(scan: &mut Scan, mut dictionary: Dictionary, limits: Limits) -> Result<()> {
    scan.trailers = scan
        .trailers
        .checked_add(1)
        .filter(|&n| n <= limits.revisions)
        .ok_or(Error::limit("reconstruction trailer budget exceeded"))?;
    // Incremental trailers can omit unchanged entries. Preserve the newest
    // explicit value and inherit only absent document-identification fields.
    for key in [b"Root".as_slice(), b"Info", b"Encrypt", b"ID"] {
        if !dictionary.contains_key(key)
            && let Some(value) = scan.table.trailer.get(key)
        {
            dictionary.insert(Name::new(key), value.clone());
        }
    }
    scan.table.trailer = dictionary;
    Ok(())
}

fn object(
    scan: &mut Scan,
    parser: &mut Parser<'_>,
    id: ObjectId,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let offset = parser.offset();
    scan.objects = scan
        .objects
        .checked_add(1)
        .filter(|&n| n <= limits.objects)
        .ok_or(Error::limit("reconstruction object budget exceeded").at(parser.position()))?;
    parser.unsigned()?;
    parser.unsigned()?;
    if !parser.keyword(b"obj") {
        return Err(Error::syntax("missing object header").at(parser.position()));
    }
    let (value, _) = parser.parse_with_streams(&mut |parser, dictionary| {
        skip_direct(parser, dictionary, id, limits, diagnostics)?;
        Ok(None)
    })?;
    diagnostics.extend(
        parser
            .take_diagnostics()
            .into_iter()
            .map(|item| item.in_object(id)),
    );
    if let Some(dictionary) = value.as_dictionary() {
        // Even an xref stream with a damaged body can have a usable trailer.
        if dictionary.is_name(b"Type", b"XRef") {
            remember(scan, dictionary.clone(), limits)?;
        }
        let before = parser.offset();
        if parser.keyword(b"stream") {
            parser.seek(before)?;
            if !skip_stream(parser, dictionary, limits, diagnostics)? {
                return Ok(());
            }
        }
    }
    crate::object::end(parser, limits, id, diagnostics)?;
    if value
        .as_dictionary()
        .is_some_and(|dictionary| dictionary.is_name(b"Type", b"ObjStm"))
    {
        scan.table.rebuild_streams.push(id);
    }
    scan.table.entries.insert(
        id.number,
        XrefEntry {
            generation: id.generation,
            location: Location::Offset(offset),
        },
    );
    Ok(())
}

/// Skip an embedded payload without interpreting its bytes as PDF tokens.
/// A fallback terminator needs a following dictionary/array continuation; EOF
/// cannot establish closure (7.3.8.1). Length/dependency validation runs again
/// when the complete containing object is loaded into its normal slot.
fn skip_direct(
    parser: &mut Parser<'_>,
    dictionary: &Dictionary,
    id: ObjectId,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if !parser.keyword(b"stream") {
        return Err(Error::syntax("missing direct stream keyword").in_object(id));
    }
    let start = crate::stream::start(parser, diagnostics, Some(id))?;
    let length = dictionary
        .get(b"Length")
        .and_then(Value::as_integer)
        .and_then(|length| usize::try_from(length).ok());
    if length.is_some_and(|length| length > limits.stream_bytes) {
        return Err(Error::limit("stream size budget exceeded").in_object(id));
    }
    let declared = length.and_then(|length| start.checked_add(length));
    if let Some(end) = declared.filter(|&end| end <= parser.data().len()) {
        parser.seek(end)?;
        if parser.keyword(b"endstream") {
            return Ok(());
        }
    }
    let (marker, _) =
        crate::object::terminator_in(parser.data(), limits, id, start, parser.data().len(), true)?;
    if let Some(end) = declared {
        diagnostics.repair(
            Diagnostic::new(
                tags::LENGTH_CORRECTED,
                "direct stream Length corrected to a validated terminator",
            )
            .at(Position::File(end))
            .in_object(id),
        )?;
    }
    parser.seek(marker)?;
    if !parser.keyword(b"endstream") {
        return Err(Error::syntax("missing endstream").in_object(id));
    }
    Ok(())
}

/// Locate a payload's end without tokenizing binary data as PDF syntax.
fn skip_stream(
    parser: &mut Parser<'_>,
    dictionary: &Dictionary,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<bool> {
    let before = parser.offset();
    let mut exact = Parser::at(parser.data(), before, Origin::File, limits)?;
    let mut attempt = Diagnostics::new(diagnostics.mode());
    match crate::xref_stream::direct_stream(&mut exact, dictionary, limits, &mut attempt) {
        Ok(_) => {
            diagnostics.extend(attempt.take());
            parser.seek(exact.offset())?;
            return Ok(true);
        }
        Err(error) if crate::xref::structural(&error) => {}
        Err(error) => return Err(error),
    }
    if !parser.keyword(b"stream") {
        return Err(Error::syntax("missing stream keyword").at(parser.position()));
    }
    let start = parser.offset();
    let end = start
        .saturating_add(limits.stream_bytes)
        .saturating_add(12)
        .min(parser.data().len());
    let bytes = parser
        .data()
        .get(start..end)
        .ok_or(Error::syntax("invalid stream extent"))?;
    for (relative, _) in bytes
        .windows(9)
        .enumerate()
        .filter(|(_, part)| *part == b"endstream")
    {
        let at = start + relative;
        let line_boundary = relative == 0 || matches!(bytes.get(relative - 1), Some(b'\n' | b'\r'));
        let mut probe = Parser::at(parser.data(), at, Origin::File, limits)?;
        if probe.keyword(b"endstream") {
            let after = probe.offset();
            if probe.keyword(b"endobj") {
                if !line_boundary {
                    probe.peek();
                    let next = probe.offset();
                    if !probe.at_end()
                        && header(parser.data(), next, limits).is_none()
                        && !probe.keyword(b"xref")
                        && !probe.keyword(b"trailer")
                    {
                        continue;
                    }
                    diagnostics.repair(Diagnostic::new(tags::STREAM_BOUNDARY, "stream resynchronized at endstream/endobj without a preceding EOL during xref reconstruction").at(Position::File(at)))?;
                }
                parser.seek(after)?;
                return Ok(true);
            }
        }
    }
    if end != parser.data().len() {
        return Err(
            Error::limit("reconstruction stream scan budget exceeded").at(parser.position())
        );
    }
    parser.seek(end)?;
    Ok(false)
}

/// Object-stream payloads are decoded after the recovered Encrypt/ID fields
/// have authenticated, so encrypted rebuilt files use the same loading path.
pub(crate) fn load_streams(
    document: &mut crate::Document,
    streams: Vec<ObjectId>,
    mut remaining: usize,
) -> Result<()> {
    let mut visited = std::collections::BTreeSet::new();
    for id in streams {
        if !visited.insert(id) || !document.slots.contains_key(&id) {
            continue;
        }
        if !document
            .object(id)?
            .dictionary()
            .is_some_and(|dictionary| dictionary.is_name(b"Type", b"ObjStm"))
        {
            continue;
        }
        let had_cache = document.object_streams.contains_key(&id.number);
        let cached = document
            .object_streams
            .remove(&id.number)
            .and_then(std::sync::OnceLock::into_inner)
            .transpose()?;
        let already_charged = cached.is_some();
        let mut header_diagnostics = Diagnostics::new(document.recovery());
        let contents = if let Some(contents) = cached {
            contents
        } else {
            let object = document.object(id)?;
            let dictionary = object
                .dictionary()
                .ok_or_else(|| Error::syntax("object stream must be a dictionary").in_object(id))?;
            crate::object_stream::headers(
                document.stream_data(object)?.into_owned(),
                dictionary,
                Limits {
                    objects: remaining,
                    ..document.limits()
                },
                id,
                &mut header_diagnostics,
            )?
        };
        document.report(header_diagnostics.take());
        let mut reservation = document.retained.reservation();
        if !already_charged {
            reservation.charge(contents.retained_bytes(), id)?;
        }
        if !had_cache {
            reservation.charge(crate::memory::stream_slot_bytes(), id)?;
        }
        remaining = remaining
            .checked_sub(contents.len())
            .ok_or(Error::limit("reconstruction object budget exceeded").in_object(id))?;
        let physical = physical_offset(document, id)?;
        for (number, index) in contents.identities() {
            // The last physical definition wins, including definitions in
            // object streams; generations remain document-relative identities.
            if number == id.number
                || document
                    .trailer()
                    .get(b"Encrypt")
                    .and_then(Value::as_reference)
                    .is_some_and(|encrypt| encrypt.number == number)
            {
                return Err(Error::syntax(
                    "object stream cannot contain itself or the encryption dictionary",
                )
                .in_object(id));
            }
            let old = document
                .slots
                .range(ObjectId::new(number, 0)..=ObjectId::new(number, u16::MAX))
                .next()
                .map(|(&id, _)| id);
            if let Some(old) = old {
                if physical_offset(document, old)? > physical {
                    continue;
                }
                if let Some(slot) = document.slots.remove(&old)
                    && let Some(Ok(object)) = slot.object.get()
                {
                    document.retained.release(object.retained_bytes());
                }
            } else {
                reservation.charge(crate::memory::slot_bytes(), ObjectId::new(number, 0))?;
            }
            document.slots.insert(
                ObjectId::new(number, 0),
                crate::Slot {
                    location: Location::Compressed {
                        stream: id.number,
                        index,
                    },
                    object: std::sync::OnceLock::new(),
                },
            );
        }
        document
            .object_streams
            .insert(id.number, std::sync::OnceLock::from(Ok(contents)));
        reservation.commit();
    }
    Ok(())
}

fn physical_offset(document: &crate::Document, id: ObjectId) -> Result<usize> {
    match document.slots.get(&id).map(|slot| slot.location) {
        Some(Location::Offset(offset)) => Ok(offset),
        Some(Location::Compressed { stream, .. }) => match document
            .slots
            .get(&ObjectId::new(stream, 0))
            .map(|slot| slot.location)
        {
            Some(Location::Offset(offset)) => Ok(offset),
            _ => Err(Error::syntax("invalid reconstructed object stream").in_object(id)),
        },
        _ => Err(Error::missing_object("missing reconstructed object").in_object(id)),
    }
}

pub(crate) fn catalog(document: &mut crate::Document) -> Result<()> {
    let mut candidate = None;
    for id in document.object_ids() {
        match document.object(id) {
            Ok(object) => {
                let Some(dictionary) = object.dictionary() else {
                    continue;
                };
                if !crate::catalog::candidate(document, dictionary)? {
                    continue;
                }
                let index = match object.source() {
                    crate::ObjectSource::Compressed { index, .. } => u64::from(*index) + 1,
                    crate::ObjectSource::File(_) => 0,
                };
                let order = (physical_offset(document, id)?, index);
                if candidate.is_none_or(|(previous, _)| order > previous) {
                    candidate = Some((order, id));
                }
            }
            Err(error) if !crate::xref::structural(&error) => return Err(error),
            _ => {}
        }
    }
    let (_, id) = candidate.ok_or(Error::syntax("reconstruction found no Catalog"))?;
    let mut diagnostics = Diagnostics::new(document.recovery());
    diagnostics.repair(
        Diagnostic::new(
            tags::TRAILER_RECONSTRUCTED,
            "unusable trailer Root reconstructed from the last Catalog",
        )
        .in_object(id),
    )?;
    document.report(diagnostics.take());
    let before = document.trailer.retained_bytes();
    document
        .trailer
        .insert(Name::new(b"Root"), Value::Reference(id));
    let after = document.trailer.retained_bytes();
    if after >= before {
        document.retained.charge(after - before, id)?;
    } else {
        document.retained.release(before - after);
    }
    Ok(())
}
