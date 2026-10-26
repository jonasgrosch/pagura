//! File structure and cross-reference data (ISO 32000-2 clauses 7.5.2–7.5.8).

mod boundary;
mod chain;
mod loading;
pub(crate) use loading::Loader;
#[cfg(test)]
pub(crate) use loading::Primary;
pub(crate) mod rebuild;
pub(crate) mod stream;

use crate::source::{ReadFault, ReadResult, View};
use crate::{Validation, repairs};
use pagura_core::syntax::{ParseMode, Parser, is_whitespace};
use pagura_core::{Diagnostics, Dictionary, Error, ErrorKind, Limits, ObjectId, Position, Result};
use std::collections::BTreeMap;

/// A misplaced object header may be this many bytes before or after its
/// cross-reference offset (ADR 0003).
pub(crate) const OFFSET_SLACK: usize = 32;
/// The eager identity check reads object headers within this many bytes.
const IDENTITY_LOOKAHEAD: usize = 1024;
/// The linearization dictionary is probed for within this many bytes of the
/// header: it is the first indirect object (F.3.3).
const LINEARIZATION_PROBE: usize = 1024;

/// Where an in-use or free object lives, per its cross-reference entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Location {
    /// The entry marks the object number as free.
    Free,
    /// `N G obj` starts at this byte offset of the file.
    Offset(usize),
    /// Stored in an object stream (cross-reference stream type 2 entry).
    Compressed {
        /// Object number of the object stream.
        stream: u32,
        /// Index within the object stream.
        index: u32,
    },
}

/// One effective cross-reference entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct XrefEntry {
    /// The object's generation number.
    pub generation: u16,
    /// Where the object lives.
    pub location: Location,
}

/// The merged cross-reference data of all revisions, keyed by object
/// number: a newer revision's entry replaces an older one.
#[derive(Debug, Clone, Default)]
pub(crate) struct XrefTable {
    pub(crate) entries: BTreeMap<u32, XrefEntry>,
    pub(crate) trailer: Dictionary,
    pub(crate) revisions: Vec<Revision>,
    pub(crate) origin: XrefOrigin,
}

/// Where cross-reference data came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum XrefOrigin {
    /// The file's own sections, following `startxref` and `Prev`.
    #[default]
    Primary,
    /// Reconstructed from indirect object definitions (ADR 0003). Object
    /// streams found by the scan still have to be read, within `remaining`
    /// more entries.
    Rebuilt {
        streams: Vec<ObjectId>,
        remaining: usize,
    },
}

/// The cross-reference representation of a revision (clauses 7.5.4–7.5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RevisionKind {
    /// A conventional cross-reference table.
    Table,
    /// A cross-reference stream.
    Stream,
    /// A table supplemented by `/XRefStm`.
    Hybrid,
}

/// Source metadata for one revision, newest first (clause 7.5.6).
/// A linearized base combines its first-page and main sections (F.3.4, F.3.11).
#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub(crate) offset: usize,
    /// Start of the section that ends the revision: `offset`, except in a
    /// linearized base, where the main section follows the first-page one.
    pub(crate) end_section: usize,
    /// Just past the trailer dictionary or the cross-reference stream's
    /// `endobj`: where `startxref` follows in an intact revision.
    pub(crate) end: usize,
    pub(crate) kind: RevisionKind,
    pub(crate) trailer: Dictionary,
    pub(crate) entries: std::sync::Arc<BTreeMap<u32, XrefEntry>>,
    // Physical entries charged before hybrid/linearized sections are combined.
    parsed_entries: usize,
}

impl Revision {
    /// Number of cross-reference entries recorded in this revision,
    /// including free and overridden hybrid/linearized entries. Matches the
    /// reader's object budget so a subsequent update remains reopenable (7.5.6).
    pub fn entry_count(&self) -> usize {
        self.parsed_entries
    }

    /// Byte offset of the revision's primary cross-reference structure.
    pub fn offset(&self) -> usize {
        self.offset
    }
    /// The cross-reference representation.
    pub fn kind(&self) -> RevisionKind {
        self.kind
    }
    /// This revision's own trailer, before inheritance from older revisions.
    pub fn trailer(&self) -> &Dictionary {
        &self.trailer
    }
}

/// Only structural failures permit reconstruction: resource and unsupported
/// decoder errors retain their original meaning and must not bypass a budget.
pub(crate) fn structural(error: &Error) -> bool {
    matches!(
        error.kind,
        ErrorKind::Syntax | ErrorKind::Cycle | ErrorKind::MissingObject
    )
}

/// How many cross-reference entries may be read: `Limits::objects`, and no
/// more than the retained-memory budget can hold as object-table entries
/// (ADR 0012), so a tiny file cannot materialise millions of entries.
pub(crate) fn entry_budget(limits: &Limits) -> usize {
    limits
        .objects
        .min(limits.document_bytes / crate::memory::slot_bytes())
}

/// Finalizes an authoritative index under the declared opening policy.
/// Complete scope has already admitted all bytes; requested scope leaves
/// unrelated indexed identities to the ordinary object loader.
pub(crate) fn finish(
    bytes: &View,
    header: usize,
    limits: &Limits,
    validation: Validation,
    table: &mut XrefTable,
) -> ReadResult<()> {
    join_linearized_base(bytes, header, table)?;
    if validation == Validation::CompleteDocument {
        let lease = bytes.materialize()?;
        validate_identities(lease.bytes(), table, limits)?;
    }
    Ok(())
}

// F.3.4 and F.3.11: the first-page and main sections describe one base,
// linked by a forward Prev. Only the main section ends that source revision.
fn join_linearized_base(bytes: &View, header: usize, table: &mut XrefTable) -> ReadResult<()> {
    let count = table.revisions.len();
    if count < 2 || table.revisions[count - 2].offset >= table.revisions[count - 1].offset {
        return Ok(());
    }
    // Keep the existing bounded probe. Its prefix extent is known even when
    // the source is sparse; unknown suffix bytes cannot become implicit EOF.
    let end = bytes.len().min(header.saturating_add(LINEARIZATION_PROBE));
    let mut window = bytes.prefix(end)?.window(header)?;
    let linearized = window.read(|parser| {
        parser.unsigned()?;
        parser.unsigned()?;
        if !parser.keyword_with_boundary(b"obj")? {
            return Ok(false);
        }
        Ok(parser
            .parse()?
            .as_dictionary()
            .and_then(|d| d.get(b"Linearized"))
            .and_then(pagura_core::Value::as_number)
            .is_some_and(|n| n > 0.0))
    });
    match linearized {
        Ok(parsed) if parsed.value => {}
        Ok(_)
        | Err(ReadFault::Invalid(Error {
            kind: ErrorKind::Syntax,
            ..
        })) => return Ok(()),
        Err(error) => return Err(error),
    }
    if let Some(main) = table.revisions.pop()
        && let Some(first_page) = table.revisions.last_mut()
    {
        let mut entries = main.entries;
        std::sync::Arc::make_mut(&mut entries)
            .append(std::sync::Arc::make_mut(&mut first_page.entries));
        first_page.entries = entries;
        first_page.parsed_entries = first_page
            .parsed_entries
            .saturating_add(main.parsed_entries);
        for (key, value) in main.trailer {
            if !first_page.trailer.contains_key(key.as_bytes()) {
                first_page.trailer.insert(key, value);
            }
        }
        first_page.end_section = main.offset;
        first_page.end = main.end;
    }
    Ok(())
}

fn validate_identities(bytes: &[u8], table: &XrefTable, limits: &Limits) -> Result<()> {
    // Verify identities, not bodies, before handing entries to the lazy cache.
    // An offset pointing to another object must trigger the same full reconstruction as a
    // malformed table (7.5.4); keep the existing nearby-offset repair path.
    for (&number, entry) in &table.entries {
        if let Location::Offset(at) = entry.location {
            let id = ObjectId::new(number, entry.generation);
            // This eager check uses a fixed-size lookahead. Longer headers
            // remain lazy, avoiding repeated scans of hostile overlapping tokens.
            let header = |offset: usize| {
                let window = bytes.len().min(offset.saturating_add(IDENTITY_LOOKAHEAD));
                bytes
                    .get(..window)
                    .and_then(|bytes| crate::object::recovery::header(bytes, offset, limits))
            };
            if header(at).is_some_and(|found| found != id)
                && !nearby_tokens(bytes, at).any(|offset| header(offset) == Some(id))
            {
                return Err(
                    Error::syntax("cross-reference offset has no matching object header")
                        .at(Position::File(at))
                        .in_object(id)
                        .with_repair(repairs::XREF_REBUILT),
                );
            }
        }
    }
    Ok(())
}

/// Offsets within [`OFFSET_SLACK`] bytes of `offset` that start a token: the
/// candidates for a misplaced object header (ADR 0003).
pub(crate) fn nearby_tokens(bytes: &[u8], offset: usize) -> impl Iterator<Item = usize> + '_ {
    let end = offset.saturating_add(OFFSET_SLACK + 1).min(bytes.len());
    (offset.saturating_sub(OFFSET_SLACK)..end).filter(|&at| {
        at.checked_sub(1)
            .is_none_or(|before| bytes.get(before).is_some_and(|&byte| is_whitespace(byte)))
            && bytes.get(at).is_some_and(u8::is_ascii_digit)
    })
}

fn merge_entries(
    table: &mut XrefTable,
    mut entries: BTreeMap<u32, XrefEntry>,
    trailer: &Dictionary,
    offset: usize,
    shift: usize,
) -> Result<BTreeMap<u32, XrefEntry>> {
    let size = integer_offset(trailer, b"Size")?.ok_or(Error::syntax("trailer Size is missing"))?;
    if size == 0
        || entries
            .keys()
            .any(|&number| u64::from(number) >= size as u64)
    {
        return Err(
            Error::syntax("trailer Size does not cover cross-reference entries")
                .at(Position::File(offset)),
        );
    }
    // The newest section already owns a complete ordered map. With physical
    // offsets the effective table copies it whole instead of inserting each key.
    if table.entries.is_empty() && shift == 0 {
        table.entries.clone_from(&entries);
        return Ok(entries);
    }
    for (&number, entry) in &mut entries {
        if let Location::Offset(at) = entry.location {
            entry.location = Location::Offset(
                at.checked_add(shift)
                    .ok_or(Error::syntax("object offset overflow"))?,
            );
        }
        table.entries.entry(number).or_insert(*entry);
    }
    Ok(entries)
}

/// One cross-reference section and the trailer that ends it.
#[derive(Debug)]
pub(crate) struct Section {
    pub(crate) entries: BTreeMap<u32, XrefEntry>,
    pub(crate) trailer: Dictionary,
    pub(crate) kind: RevisionKind,
    /// Just past the trailer dictionary or the stream's `endobj`.
    pub(crate) end: usize,
}

/// Shared section grammar. Source extents are checked against the declared
/// file length, never against the size of a temporary parser window. `end`
/// remains relative to this parser's input, as `Parser::offset` is.
fn parse_section<M: ParseMode>(
    parser: &mut Parser<'_, M>,
    file_length: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<Section, M::Fault> {
    let start = parser.offset();
    if !parser.keyword_with_boundary(b"xref")? {
        parser.seek(start)?;
        return stream::parse(parser, file_length, limits, diagnostics);
    }
    let entries = classic_section(file_length, parser, limits)?;
    let trailer = parser
        .parse()?
        .as_dictionary()
        .cloned()
        .ok_or(Error::syntax("trailer must be a dictionary").at(parser.position()))?;
    diagnostics.absorb(parser.take_diagnostics())?;
    Ok(Section {
        entries,
        trailer,
        kind: RevisionKind::Table,
        end: parser.offset(),
    })
}

/// The same bounded-window probe retains undecided tokens as incomplete.
/// A caller probing an alternative offset may classify terminal Syntax as
/// a known mismatch; required section parsing still reports that failure.
fn section_header<M: ParseMode>(parser: &mut Parser<'_, M>) -> std::result::Result<bool, M::Fault> {
    if parser.keyword_with_boundary(b"xref")? {
        return Ok(true);
    }
    parser.unsigned()?;
    parser.unsigned()?;
    parser.keyword_with_boundary(b"obj")
}

pub(crate) fn integer_offset(dictionary: &Dictionary, key: &[u8]) -> Result<Option<usize>> {
    dictionary
        .get(key)
        .map(|value| {
            value
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(Error::syntax(
                    "cross-reference parameter must be a nonnegative integer",
                ))
        })
        .transpose()
}

/// Reads the subsections of a classic table, up to `trailer` (clause 7.5.4).
fn classic_section<M: ParseMode>(
    file_length: usize,
    parser: &mut Parser<'_, M>,
    limits: &Limits,
) -> std::result::Result<BTreeMap<u32, XrefEntry>, M::Fault> {
    let mut entries = BTreeMap::new();
    while !parser.keyword_with_boundary(b"trailer")? {
        let invalid =
            |parser: &Parser<'_, M>, message| Error::syntax(message).at(parser.position());
        let first = u32::try_from(parser.unsigned()?)
            .map_err(|_| invalid(parser, "object number out of range"))?;
        let count = usize::try_from(parser.unsigned()?)
            .map_err(|_| invalid(parser, "subsection length out of range"))?;
        if entries.len().saturating_add(count) > limits.objects {
            return Err(Error::limit("cross-reference entry budget exceeded")
                .at(parser.position())
                .into());
        }
        for index in 0..count {
            let number = u32::try_from(index)
                .ok()
                .and_then(|index| first.checked_add(index))
                .ok_or_else(|| invalid(parser, "object number out of range"))?;
            let offset = usize::try_from(parser.unsigned()?)
                .map_err(|_| invalid(parser, "object offset out of range"))?;
            let generation = u16::try_from(parser.unsigned()?)
                .map_err(|_| invalid(parser, "generation out of range"))?;
            let kind = parser.word()?;
            let location = match kind {
                b"n" if number == 0 => {
                    return Err(invalid(parser, "object 0 cannot be in use").into());
                }
                b"n" if offset >= file_length => {
                    return Err(invalid(parser, "object offset outside the file").into());
                }
                b"n" => Location::Offset(offset),
                b"f" => Location::Free,
                _ => return Err(invalid(parser, "entry type must be n or f").into()),
            };
            if entries
                .insert(
                    number,
                    XrefEntry {
                        generation,
                        location,
                    },
                )
                .is_some()
            {
                return Err(invalid(parser, "subsections overlap").into());
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;
