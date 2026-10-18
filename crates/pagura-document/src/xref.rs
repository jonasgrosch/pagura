//! File structure and cross-reference data (ISO 32000-2 clauses 7.5.2–7.5.8).

use crate::tags;
use pagura_core::syntax::{Origin, Parser, is_whitespace};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Limits, Position, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Where an in-use or free object lives, per its cross-reference entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Location {
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
pub struct XrefEntry {
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
    pub(crate) rebuild_streams: Vec<pagura_core::ObjectId>,
    pub(crate) rebuild_remaining: Option<usize>,
    pub(crate) revisions: Vec<Revision>,
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
#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub(crate) offset: usize,
    pub(crate) kind: RevisionKind,
    pub(crate) trailer: Dictionary,
}

impl Revision {
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

/// Reads header, trailers and newest-wins cross-reference data.
pub(crate) fn read(
    bytes: &[u8],
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<XrefTable> {
    let header = check_header(bytes, diagnostics)?;
    let mut attempt = Diagnostics::new(diagnostics.mode());
    match primary(bytes, header, limits, &mut attempt) {
        Ok(table) => {
            diagnostics.extend(attempt.take());
            Ok(table)
        }
        Err(error) if diagnostics.mode() == pagura_core::Recovery::Repair && structural(&error) => {
            crate::rebuild::read(bytes, limits, diagnostics, error.position)
        }
        Err(error) => Err(error),
    }
}

/// Only structural failures permit reconstruction: resource and unsupported
/// decoder errors retain their original meaning and must not bypass a budget.
pub(crate) fn structural(error: &Error) -> bool {
    matches!(
        error.kind,
        pagura_core::ErrorKind::Syntax
            | pagura_core::ErrorKind::Cycle
            | pagura_core::ErrorKind::MissingObject
    )
}

fn primary(
    bytes: &[u8],
    header: usize,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<XrefTable> {
    let start = find_startxref(bytes, limits, diagnostics)?.ok_or(
        Error::syntax("missing startxref near the end of the file").at(Position::File(bytes.len())),
    )?;
    // Leading junk can either be included in the recorded offsets or have
    // been prepended afterwards. Never shift a usable physical offset.
    let shifted = header != 0
        && !is_section(bytes, start, limits)
        && start
            .checked_add(header)
            .is_some_and(|at| is_section(bytes, at, limits));
    let shift = if shifted { header } else { 0 };
    let mut offset = start
        .checked_add(shift)
        .ok_or(Error::syntax("xref offset overflow"))?;
    let mut visited = BTreeSet::new();
    let mut table = XrefTable::default();
    let mut remaining = limits.objects;
    loop {
        if !visited.insert(offset) {
            return Err(Error::new(
                pagura_core::ErrorKind::Cycle,
                "cross-reference revision cycle",
            )
            .at(Position::File(offset)));
        }
        if table.revisions.len() >= limits.revisions {
            return Err(Error::limit("revision budget exceeded").at(Position::File(offset)));
        }
        let (mut entries, trailer, mut kind) = section(
            bytes,
            offset,
            Limits {
                objects: remaining,
                ..limits
            },
            diagnostics,
        )?;
        charge(&mut remaining, entries.len())?;
        if kind == RevisionKind::Table
            && let Some(hybrid) = integer_offset(&trailer, b"XRefStm")?
        {
            let hybrid = hybrid
                .checked_add(shift)
                .ok_or(Error::syntax("xref offset overflow"))?;
            if !visited.insert(hybrid) {
                return Err(Error::new(
                    pagura_core::ErrorKind::Cycle,
                    "hybrid cross-reference cycle",
                )
                .at(Position::File(hybrid)));
            }
            let (supplement, _) = crate::xref_stream::read(
                bytes,
                hybrid,
                Limits {
                    objects: remaining,
                    ..limits
                },
                diagnostics,
            )?;
            charge(&mut remaining, supplement.len())?;
            // Clause 7.5.8.4: stream entries override the companion table.
            entries.extend(supplement);
            kind = RevisionKind::Hybrid;
        }
        merge_entries(&mut table, entries, &trailer, offset, shift)?;
        for (key, value) in trailer.iter() {
            if !table.trailer.contains_key(key.as_bytes()) {
                table.trailer.insert(key.clone(), value.clone());
            }
        }
        let prev = integer_offset(&trailer, b"Prev")?;
        table.revisions.push(Revision {
            offset,
            kind,
            trailer,
        });
        let Some(prev) = prev else {
            break;
        };
        offset = prev
            .checked_add(shift)
            .ok_or(Error::syntax("Prev offset overflow"))?;
    }
    validate_identities(bytes, &table, limits)?;
    Ok(table)
}

fn validate_identities(bytes: &[u8], table: &XrefTable, limits: Limits) -> Result<()> {
    // Verify identities, not bodies, before handing entries to the lazy cache.
    // An offset pointing to another object must trigger the same full reconstruction as a
    // malformed table (7.5.4); keep the existing nearby-offset repair path.
    for (&number, entry) in &table.entries {
        if let Location::Offset(at) = entry.location {
            let id = pagura_core::ObjectId::new(number, entry.generation);
            // This eager check uses a fixed-size lookahead. Longer headers
            // remain lazy, avoiding repeated scans of hostile overlapping tokens.
            let header = |offset: usize| {
                crate::object::header(
                    &bytes[..offset.saturating_add(1024).min(bytes.len())],
                    offset,
                    limits,
                )
            };
            let matches = |offset| header(offset) == Some(id);
            if header(at).is_some()
                && !matches(at)
                && !(at.saturating_sub(32)..at.saturating_add(33).min(bytes.len())).any(|offset| {
                    (offset == 0
                        || bytes
                            .get(offset - 1)
                            .is_some_and(|&byte| is_whitespace(byte)))
                        && matches(offset)
                })
            {
                let mut error =
                    Error::syntax("cross-reference offset has no matching object header")
                        .at(Position::File(at))
                        .in_object(id);
                error.repair = Some(tags::XREF_REBUILT);
                return Err(error);
            }
        }
    }
    Ok(())
}

fn merge_entries(
    table: &mut XrefTable,
    entries: BTreeMap<u32, XrefEntry>,
    trailer: &Dictionary,
    offset: usize,
    shift: usize,
) -> Result<()> {
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
    for (number, mut entry) in entries {
        if let Location::Offset(at) = entry.location {
            entry.location = Location::Offset(
                at.checked_add(shift)
                    .ok_or(Error::syntax("object offset overflow"))?,
            );
        }
        table.entries.entry(number).or_insert(entry);
    }
    Ok(())
}

fn section(
    bytes: &[u8],
    offset: usize,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<(BTreeMap<u32, XrefEntry>, Dictionary, RevisionKind)> {
    let mut parser =
        Parser::at(bytes, offset, Origin::File, limits)?.with_recovery(diagnostics.mode());
    let result = if parser.keyword(b"xref") {
        let entries = classic_section(bytes, &mut parser, limits)?;
        let trailer = parser
            .parse()?
            .as_dictionary()
            .cloned()
            .ok_or(Error::syntax("trailer must be a dictionary").at(parser.position()))?;
        diagnostics.extend(parser.take_diagnostics());
        (entries, trailer, RevisionKind::Table)
    } else {
        let (entries, trailer) = crate::xref_stream::read(bytes, offset, limits, diagnostics)?;
        (entries, trailer, RevisionKind::Stream)
    };
    Ok(result)
}

fn charge(remaining: &mut usize, count: usize) -> Result<()> {
    *remaining = remaining
        .checked_sub(count)
        .ok_or(Error::limit("cross-reference entry budget exceeded"))?;
    Ok(())
}

fn is_section(bytes: &[u8], offset: usize, limits: Limits) -> bool {
    let Ok(mut parser) = Parser::at(bytes, offset, Origin::File, limits) else {
        return false;
    };
    parser.keyword(b"xref")
        || (parser.unsigned().is_ok() && parser.unsigned().is_ok() && parser.keyword(b"obj"))
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

fn check_header(bytes: &[u8], diagnostics: &mut Diagnostics) -> Result<usize> {
    let offset = bytes
        .get(..bytes.len().min(1032))
        .and_then(|head| {
            head.windows(8).position(|part| {
                part.starts_with(b"%PDF-")
                    // The header grammar permits any single minor digit for
                    // major versions 1 and 2 (ISO 32000-2:2020, 7.5.2).
                    && matches!(part[5], b'1' | b'2')
                    && part[6] == b'.'
                    && part[7].is_ascii_digit()
            })
        })
        .filter(|&at| at < 1024)
        .ok_or(Error::syntax("missing or unknown PDF header").at(Position::File(0)))?;
    if offset != 0 {
        diagnostics.repair(
            Diagnostic::new(
                tags::HEADER_OFFSET,
                "PDF header follows leading junk; offsets adjusted when necessary",
            )
            .at(Position::File(offset)),
        )?;
    }
    if !matches!(bytes.get(offset + 8), Some(b'\r' | b'\n')) {
        let suffix = bytes
            .get(offset + 8..)
            .ok_or(Error::syntax("truncated PDF header"))?;
        if !suffix
            .iter()
            .take(1024)
            .any(|&byte| matches!(byte, b'\r' | b'\n'))
        {
            return Err(Error::syntax("missing or unknown PDF header").at(Position::File(offset)));
        }
        diagnostics.repair(
            Diagnostic::new(
                tags::HEADER_FORMAT,
                "extra bytes in the PDF header line; version retained",
            )
            .at(Position::File(offset + 8)),
        )?;
    }
    Ok(offset)
}

/// Finds the offset given after the last `startxref` (clause 7.5.5).
fn find_startxref(
    bytes: &[u8],
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Option<usize>> {
    let window = bytes.len().saturating_sub(1024);
    let Some(marker) = bytes[window..]
        .windows(9)
        .rposition(|part| part == b"startxref")
        .map(|index| window + index)
    else {
        return Ok(None);
    };
    let mut tail = Parser::at(bytes, marker + 9, Origin::File, limits)?;
    let offset = usize::try_from(tail.unsigned()?)
        .ok()
        .filter(|&offset| offset < bytes.len())
        .ok_or(Error::syntax("startxref offset outside the file").at(Position::File(marker)))?;
    let rest = &bytes[tail.offset()..];
    let rest = &rest[rest.iter().take_while(|&&b| is_whitespace(b)).count()..];
    let Some(after_eof) = rest.strip_prefix(b"%%EOF") else {
        return Err(Error::syntax("missing %%EOF after startxref").at(tail.position()));
    };
    if !after_eof.iter().all(|&b| is_whitespace(b)) {
        diagnostics.repair(
            Diagnostic::new(
                tags::TRAILING_BYTES,
                "data follows the final %%EOF; ignored",
            )
            .at(Position::File(bytes.len() - after_eof.len())),
        )?;
    }
    Ok(Some(offset))
}

/// Reads the subsections of a classic table, up to `trailer` (clause 7.5.4).
fn classic_section(
    bytes: &[u8],
    parser: &mut Parser<'_>,
    limits: Limits,
) -> Result<BTreeMap<u32, XrefEntry>> {
    let mut entries = BTreeMap::new();
    while !parser.keyword(b"trailer") {
        let invalid = |parser: &Parser<'_>, message| Error::syntax(message).at(parser.position());
        let first = u32::try_from(parser.unsigned()?)
            .map_err(|_| invalid(parser, "object number out of range"))?;
        let count = usize::try_from(parser.unsigned()?)
            .map_err(|_| invalid(parser, "subsection length out of range"))?;
        if entries.len().saturating_add(count) > limits.objects {
            return Err(Error::limit("cross-reference entry budget exceeded").at(parser.position()));
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
                b"n" if number == 0 => return Err(invalid(parser, "object 0 cannot be in use")),
                b"n" if offset >= bytes.len() => {
                    return Err(invalid(parser, "object offset outside the file"));
                }
                b"n" => Location::Offset(offset),
                b"f" => Location::Free,
                _ => return Err(invalid(parser, "entry type must be n or f")),
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
                return Err(invalid(parser, "subsections overlap"));
            }
        }
    }
    Ok(entries)
}
