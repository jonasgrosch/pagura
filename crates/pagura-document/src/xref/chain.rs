//! One owned newest-wins revision traversal, independent of byte availability.

use super::{
    Revision, RevisionKind, Section, XrefTable, entry_budget, integer_offset, merge_entries,
};
use pagura_core::{Error, Limits, Meter, Position, Result};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Section,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) offset: usize,
    pub(super) objects: usize,
    pub(super) kind: Kind,
}

#[derive(Debug)]
struct Primary {
    section: Section,
    offset: usize,
    available: usize,
}

/// A primary section stays owned while its hybrid supplement is unavailable.
/// Call `accept` only after the current request completed; a wait changes
/// neither the request nor the entry/revision/cycle budgets.
#[derive(Debug)]
pub(super) struct Chain {
    table: XrefTable,
    visited: BTreeSet<usize>,
    entries: Meter,
    revisions: usize,
    shift: usize,
    request: Request,
    primary: Option<Primary>,
    finished: bool,
}

impl Chain {
    pub(super) fn new(offset: usize, shift: usize, limits: &Limits) -> Result<Self> {
        let entries = Meter::new(
            entry_budget(limits),
            "cross-reference entry budget exceeded",
        );
        let mut chain = Self {
            table: XrefTable::default(),
            visited: BTreeSet::new(),
            request: Request {
                offset,
                objects: entries.remaining(),
                kind: Kind::Section,
            },
            entries,
            revisions: limits.revisions,
            shift,
            primary: None,
            finished: false,
        };
        chain.begin(offset)?;
        Ok(chain)
    }

    pub(super) fn request(&self) -> Option<Request> {
        (!self.finished).then_some(self.request)
    }

    /// Publishes one completed section into this private bootstrap attempt.
    /// A terminal error ends the caller's attempt; it must not be retried.
    pub(super) fn accept(&mut self, section: Section) -> Result<()> {
        debug_assert!(!self.finished);
        self.entries.charge(section.entries.len())?;
        if let Some(mut primary) = self.primary.take() {
            debug_assert_eq!(self.request.kind, Kind::Stream);
            debug_assert_eq!(section.kind, RevisionKind::Stream);
            // Clause 7.5.8.4: the stream overrides its companion table.
            primary.section.entries.extend(section.entries);
            primary.section.kind = RevisionKind::Hybrid;
            return self.publish(primary);
        }
        let primary = Primary {
            section,
            offset: self.request.offset,
            available: self.request.objects,
        };
        if primary.section.kind == RevisionKind::Table
            && let Some(hybrid) = integer_offset(&primary.section.trailer, b"XRefStm")?
        {
            let hybrid = hybrid
                .checked_add(self.shift)
                .ok_or(Error::syntax("xref offset overflow"))?;
            if !self.visited.insert(hybrid) {
                return Err(Error::cycle("hybrid cross-reference cycle").at(Position::File(hybrid)));
            }
            self.request = Request {
                offset: hybrid,
                objects: self.entries.remaining(),
                kind: Kind::Stream,
            };
            self.primary = Some(primary);
            return Ok(());
        }
        self.publish(primary)
    }

    pub(super) fn into_table(self) -> XrefTable {
        debug_assert!(self.finished);
        self.table
    }

    fn begin(&mut self, offset: usize) -> Result<()> {
        if !self.visited.insert(offset) {
            return Err(Error::cycle("cross-reference revision cycle").at(Position::File(offset)));
        }
        if self.table.revisions.len() >= self.revisions {
            return Err(Error::limit("revision budget exceeded").at(Position::File(offset)));
        }
        self.request = Request {
            offset,
            objects: self.entries.remaining(),
            kind: Kind::Section,
        };
        Ok(())
    }

    fn publish(&mut self, primary: Primary) -> Result<()> {
        let Primary {
            section,
            offset,
            available,
        } = primary;
        let Section {
            entries,
            trailer,
            kind,
            end,
        } = section;
        let entries = merge_entries(&mut self.table, entries, &trailer, offset, self.shift)?;
        for (key, value) in &trailer {
            if !self.table.trailer.contains_key(key.as_bytes()) {
                self.table.trailer.insert(key.clone(), value.clone());
            }
        }
        let previous = integer_offset(&trailer, b"Prev")?;
        self.table.revisions.push(Revision {
            offset,
            end_section: offset,
            end,
            kind,
            trailer,
            entries: Arc::new(entries),
            parsed_entries: available.saturating_sub(self.entries.remaining()),
        });
        if let Some(previous) = previous {
            let offset = previous
                .checked_add(self.shift)
                .ok_or(Error::syntax("Prev offset overflow"))?;
            self.begin(offset)
        } else {
            self.finished = true;
            Ok(())
        }
    }
}
