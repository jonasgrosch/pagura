//! Original classic records retained only after a proven Size-coverage fault.
//!
//! These are source evidence, never a valid revision or an alternate index.

use super::{IDENTITY_LOOKAHEAD, Location, XrefEntry};
use crate::memory;
use pagura_core::convert::usize_to_u64;
use pagura_core::syntax::{Origin, Parser, is_whitespace};
use pagura_core::{Error, ErrorKind, INDEX_ENTRY_OVERHEAD, Limits, ObjectId, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Exact source records and their physical upper bounds, with one retained charge.
#[derive(Debug)]
pub(crate) struct OriginalRecords {
    entries: BTreeMap<u32, XrefEntry>,
    boundaries: Vec<usize>,
    retained: Arc<memory::Budget>,
}

impl OriginalRecords {
    /// Validate before retaining anything. Other source faults supply no authority.
    pub(crate) fn retain(
        bytes: &[u8],
        entries: BTreeMap<u32, XrefEntry>,
        section: usize,
        end: usize,
        declared_size: usize,
        limits: &Limits,
    ) -> Result<Option<Arc<Self>>> {
        let Some((&largest, _)) = entries.last_key_value() else {
            return Ok(None);
        };
        if declared_size == 0
            || u64::from(largest) < usize_to_u64(declared_size)
            || section >= end
            || end > bytes.len()
        {
            return Ok(None);
        }
        let mut physical = 0_usize;
        for (&number, entry) in &entries {
            if matches!(entry.location, Location::Compressed { .. }) {
                return Ok(None);
            }
            if let Location::Offset(offset) = entry.location {
                if !exact_header(
                    bytes,
                    offset,
                    ObjectId::new(number, entry.generation),
                    section,
                    limits,
                )? {
                    return Ok(None);
                }
                physical = physical.saturating_add(1);
            }
        }
        if physical == 0 {
            return Ok(None);
        }
        let capacity = physical.saturating_add(1);
        let retained = Arc::new(memory::Budget::new(limits.document_bytes));
        let header = size_of::<Self>()
            .saturating_add(2_usize.saturating_mul(size_of::<usize>()))
            .saturating_add(size_of::<Arc<Self>>());
        let records = entries
            .len()
            .saturating_mul(size_of::<(u32, XrefEntry)>().saturating_add(INDEX_ENTRY_OVERHEAD));
        retained.charge(
            header
                .saturating_add(records)
                .saturating_add(capacity.saturating_mul(size_of::<usize>())),
            None,
        )?;
        let mut boundaries = Vec::new();
        boundaries
            .try_reserve_exact(capacity)
            .map_err(|_| Error::limit("original record boundary allocation failed"))?;
        retained.charge(
            boundaries
                .capacity()
                .saturating_sub(capacity)
                .saturating_mul(size_of::<usize>()),
            None,
        )?;
        boundaries.extend(entries.values().filter_map(|entry| match entry.location {
            Location::Offset(offset) => Some(offset),
            Location::Free | Location::Compressed { .. } => None,
        }));
        boundaries.push(section);
        boundaries.sort_unstable();
        Ok(Some(Arc::new(Self {
            entries,
            boundaries,
            retained,
        })))
    }

    /// Only this exact original identity/offset may use the next physical bound.
    pub(crate) fn boundary(&self, id: ObjectId, offset: usize) -> Option<usize> {
        let entry = self.entries.get(&id.number)?;
        if entry.generation != id.generation || entry.location != Location::Offset(offset) {
            return None;
        }
        self.boundaries
            .get(self.boundaries.partition_point(|&at| at <= offset))
            .copied()
    }

    /// Builder charges its ordinary slots and caches to this same aggregate.
    pub(crate) fn budget(&self) -> &Arc<memory::Budget> {
        &self.retained
    }
}

fn exact_header(
    bytes: &[u8],
    offset: usize,
    id: ObjectId,
    section: usize,
    limits: &Limits,
) -> Result<bool> {
    if offset >= section
        || !bytes.get(offset).is_some_and(u8::is_ascii_digit)
        || offset
            .checked_sub(1)
            .is_some_and(|before| !bytes.get(before).is_some_and(|&byte| is_whitespace(byte)))
    {
        return Ok(false);
    }
    // Keep a real following byte, so truncating the probe cannot close a token.
    let bound = offset.saturating_add(IDENTITY_LOOKAHEAD).min(section);
    let Some(span) = bytes.get(..bound.saturating_add(1)) else {
        return Ok(false);
    };
    let mut parser = Parser::with_offset(span, offset, Origin::File, *limits)?;
    let parsed: Result<bool> = (|| {
        let number = parser.unsigned()?;
        let generation = parser.unsigned()?;
        Ok(number == u64::from(id.number)
            && generation == u64::from(id.generation)
            && parser.keyword(b"obj")
            && parser.offset() <= bound)
    })();
    match parsed {
        Err(error) if error.kind == ErrorKind::Limit => Err(error),
        Err(_) => Ok(false),
        result => result,
    }
}
