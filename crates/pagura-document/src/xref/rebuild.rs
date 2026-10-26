//! Reported reconstruction after a structural cross-reference failure (ADR 0003).
//!
//! The scanner advances monotonically and skips strings and stream payloads.
//! Heuristics follow the ADR 0003 repair catalogue and the damage classes seen
//! in the real-world corpus. Object streams and the catalog of a rebuilt table
//! are read through a scratch document; what they establish is returned as a
//! plan that the document builder applies before any cache is kept.

use super::{Location, XrefEntry, XrefOrigin, XrefTable, entry_budget, structural};
use crate::object::recovery::{self, StreamContext, TERMINATOR_SLACK, header};
use crate::object::stream::headers;
use crate::source::{ReadFault, ReadResult};
use crate::{Document, ObjectSource, repairs};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, Meter, Name, ObjectId, Position, Result,
    Value,
};
use std::collections::{BTreeMap, BTreeSet};

struct Scan {
    table: XrefTable,
    streams: Vec<ObjectId>,
    objects: Meter,
    trailers: Meter,
}

pub(crate) fn read(
    bytes: &[u8],
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    position: Position,
) -> Result<XrefTable> {
    diagnostics.repair(
        Diagnostic::new(
            repairs::XREF_REBUILT,
            "damaged cross-reference data reconstructed from indirect object definitions",
        )
        .at(position),
    )?;
    let mut scan = Scan {
        table: XrefTable::default(),
        streams: Vec::new(),
        objects: Meter::new(
            entry_budget(limits),
            "reconstruction object budget exceeded",
        ),
        trailers: Meter::new(limits.revisions, "reconstruction trailer budget exceeded"),
    };
    let mut parser = Parser::new(bytes, Origin::File, *limits).with_recovery(diagnostics.mode());
    while !parser.at_end() {
        let offset = parser.offset();
        let result = if parser.keyword(b"trailer") {
            parser.parse().and_then(|value| match value {
                Value::Dictionary(dictionary) => remember(&mut scan, dictionary),
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
            && !structural(&error)
        {
            return Err(error);
        }
        // Parsing errors can leave the cursor at the offending token. Never
        // restart a failed parse at an earlier byte, including truncated tails.
        if parser.offset() <= offset {
            parser.seek(offset + 1)?;
        }
        diagnostics.absorb(parser.take_diagnostics())?;
    }
    if scan.table.entries.is_empty() {
        return Err(
            Error::syntax("reconstruction found no complete indirect objects").at(position),
        );
    }
    scan.table.origin = XrefOrigin::Rebuilt {
        streams: scan.streams,
        remaining: scan.objects.remaining(),
    };
    Ok(scan.table)
}

fn remember(scan: &mut Scan, mut dictionary: Dictionary) -> Result<()> {
    scan.trailers.charge(1)?;
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
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let offset = parser.offset();
    scan.objects
        .charge(1)
        .map_err(|error| error.at(parser.position()))?;
    parser.unsigned()?;
    parser.unsigned()?;
    if !parser.keyword(b"obj") {
        return Err(Error::syntax("missing object header").at(parser.position()));
    }
    let (value, _) = parser.parse_with_streams(&mut |parser, dictionary| {
        skip_direct(parser, dictionary, id, limits, diagnostics)?;
        Ok(None)
    })?;
    diagnostics.absorb(
        parser
            .take_diagnostics()
            .into_iter()
            .map(|item| item.in_object(id)),
    )?;
    if let Some(dictionary) = value.as_dictionary() {
        // Even an xref stream with a damaged body can have a usable trailer.
        if dictionary.is_name(b"Type", b"XRef") {
            remember(scan, dictionary.clone())?;
        }
        let before = parser.offset();
        if parser.keyword(b"stream") {
            parser.seek(before)?;
            if !skip_stream(parser, dictionary, limits, diagnostics)? {
                return Ok(());
            }
        }
    }
    recovery::end(parser, limits, id, diagnostics)?;
    if value
        .as_dictionary()
        .is_some_and(|dictionary| dictionary.is_name(b"Type", b"ObjStm"))
    {
        scan.streams.push(id);
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
    limits: &Limits,
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
    let (marker, _) = recovery::terminator_in(
        parser.data(),
        limits,
        id,
        start..parser.data().len(),
        StreamContext::Direct,
    )?;
    if let Some(end) = declared {
        diagnostics.repair(
            Diagnostic::new(
                repairs::LENGTH_CORRECTED,
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
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<bool> {
    let before = parser.offset();
    let mut exact = Parser::with_offset(parser.data(), before, Origin::File, *limits)?;
    let mut attempt = Diagnostics::new(diagnostics.mode());
    match super::stream::bootstrap_stream_data(&mut exact, dictionary, limits, &mut attempt) {
        Ok(_) => {
            diagnostics.absorb(attempt.take())?;
            parser.seek(exact.offset())?;
            return Ok(true);
        }
        Err(ReadFault::Invalid(error)) if structural(&error) => {}
        Err(error) => return Err(error),
    }
    if !parser.keyword(b"stream") {
        return Err(Error::syntax("missing stream keyword").at(parser.position()));
    }
    let start = parser.offset();
    // The EOL after the stream keyword is still unread here.
    let end = start
        .saturating_add(limits.stream_bytes)
        .saturating_add(TERMINATOR_SLACK + 1)
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
        let line_boundary = relative
            .checked_sub(1)
            .is_none_or(|before| matches!(bytes.get(before), Some(b'\n' | b'\r')));
        let mut probe = Parser::with_offset(parser.data(), at, Origin::File, *limits)?;
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
                    diagnostics.repair(
                        Diagnostic::new(
                            repairs::STREAM_BOUNDARY,
                            "stream resynchronized at endstream/endobj without a preceding EOL \
                             during xref reconstruction",
                        )
                        .at(Position::File(at)),
                    )?;
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

/// The object-table changes that reconstructed object streams establish.
#[derive(Debug, Default)]
pub(crate) struct StreamSlots {
    /// Compressed definitions by object number.
    pub(crate) slots: BTreeMap<u32, Replacement>,
    /// Object numbers of the object streams read.
    pub(crate) streams: Vec<u32>,
}

/// A compressed definition and the physical slot it supersedes, if any.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Replacement {
    pub(crate) replaced: Option<ObjectId>,
    pub(crate) location: Location,
}

impl StreamSlots {
    /// The slot holding object `number` once these changes apply.
    fn slot(&self, document: &Document, number: u32) -> Option<ObjectId> {
        if self.slots.contains_key(&number) {
            return Some(ObjectId::new(number, 0));
        }
        document
            .slots
            .range(ObjectId::new(number, 0)..=ObjectId::new(number, u16::MAX))
            .next()
            .map(|(&id, _)| id)
    }

    fn location(&self, document: &Document, id: ObjectId) -> Option<Location> {
        match self.slots.get(&id.number) {
            Some(replacement) => (id.generation == 0).then_some(replacement.location),
            None => document.slots.get(&id).map(|slot| slot.location),
        }
    }

    /// Where `id` is physically defined: its own offset, or its stream's.
    fn physical_offset(&self, document: &Document, id: ObjectId) -> Result<usize> {
        match self.location(document, id) {
            Some(Location::Offset(offset)) => Ok(offset),
            Some(Location::Compressed { stream, .. }) => {
                match self.location(document, ObjectId::new(stream, 0)) {
                    Some(Location::Offset(offset)) => Ok(offset),
                    _ => Err(Error::syntax("invalid reconstructed object stream").in_object(id)),
                }
            }
            _ => Err(Error::missing_object("missing reconstructed object").in_object(id)),
        }
    }
}

/// Reads the headers of reconstructed object streams. The last physical
/// definition of an object wins, including definitions in object streams;
/// generations remain document-relative identities. Object-stream payloads
/// are decoded after the recovered Encrypt/ID fields have authenticated, so
/// encrypted rebuilt files use the same loading path.
pub(crate) fn object_streams(
    document: &Document,
    streams: Vec<ObjectId>,
    remaining: usize,
) -> ReadResult<StreamSlots> {
    let mut plan = StreamSlots::default();
    let mut budget = Meter::new(remaining, "reconstruction object budget exceeded");
    let encrypt = document
        .trailer()
        .get(b"Encrypt")
        .and_then(Value::as_reference);
    let mut visited = BTreeSet::new();
    for id in streams {
        // A stream superseded by a later compressed definition is no stream.
        if !visited.insert(id)
            || plan.slots.contains_key(&id.number)
            || !document.slots.contains_key(&id)
        {
            continue;
        }
        let object = document.object(id)?;
        let Some(dictionary) = object
            .dictionary()
            .filter(|dictionary| dictionary.is_name(b"Type", b"ObjStm"))
        else {
            continue;
        };
        let mut diagnostics = Diagnostics::new(document.recovery());
        let contents = headers(
            document.stream_data(object)?.into_owned(),
            dictionary,
            &Limits {
                objects: budget.remaining(),
                ..document.limits()
            },
            id,
            &mut diagnostics,
        )?;
        document.report(diagnostics.take());
        budget
            .charge(contents.len())
            .map_err(|error| error.in_object(id))?;
        let physical = plan.physical_offset(document, id)?;
        for (number, index) in contents.identities() {
            if number == id.number || encrypt.is_some_and(|encrypt| encrypt.number == number) {
                return Err((Error::syntax(
                    "object stream cannot contain itself or the encryption dictionary",
                )
                .in_object(id))
                .into());
            }
            let old = plan.slot(document, number);
            if let Some(old) = old
                && plan.physical_offset(document, old)? > physical
            {
                continue;
            }
            let replaced = match plan.slots.get(&number) {
                Some(earlier) => earlier.replaced,
                None => old,
            };
            plan.slots.insert(
                number,
                Replacement {
                    replaced,
                    location: Location::Compressed {
                        stream: id.number,
                        index,
                    },
                },
            );
        }
        plan.streams.push(id.number);
    }
    Ok(plan)
}

/// The Root of a reconstructed document: `None` keeps the trailer's when it
/// names a usable catalog; otherwise the last physical catalog candidate,
/// reported as a repair.
pub(crate) fn root(document: &Document) -> ReadResult<Option<ObjectId>> {
    match document.catalog() {
        Ok(catalog) if crate::catalog::candidate(document, catalog)? => return Ok(None),
        Err(ReadFault::Invalid(error)) if structural(&error) => {}
        Err(fault) => return Err(fault),
        _ => {}
    }
    let mut candidate = None;
    for id in document.object_ids() {
        let object = match document.object(id) {
            Ok(object) => object,
            Err(ReadFault::Invalid(error)) if structural(&error) => continue,
            Err(fault) => return Err(fault),
        };
        let Some(dictionary) = object.dictionary() else {
            continue;
        };
        if !crate::catalog::candidate(document, dictionary)? {
            continue;
        }
        let index = match object.source() {
            ObjectSource::Compressed { index, .. } => u64::from(*index) + 1,
            _ => 0,
        };
        let order = (physical_offset(document, id)?, index);
        if candidate.is_none_or(|(previous, _)| order > previous) {
            candidate = Some((order, id));
        }
    }
    let (_, id) = candidate.ok_or(Error::syntax("reconstruction found no Catalog"))?;
    document.repair(
        Diagnostic::new(
            repairs::TRAILER_RECONSTRUCTED,
            "unusable trailer Root reconstructed from the last Catalog",
        )
        .in_object(id),
    )?;
    Ok(Some(id))
}

fn physical_offset(document: &Document, id: ObjectId) -> Result<usize> {
    StreamSlots::default().physical_offset(document, id)
}
