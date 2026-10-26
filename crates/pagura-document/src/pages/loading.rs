//! Owned selected-page lookup through the same cursor as complete enumeration.

use super::Page;
use super::directory::{Cursor, Selection};
use crate::Document;
use crate::source::{ReadFault, ReadResult};
use pagura_core::Error;
use std::sync::Arc;

/// Progress of an owned requested-page lookup.
#[derive(Debug)]
#[non_exhaustive]
pub enum PageLoading {
    /// One structural unit completed; another advance may progress.
    Yielded,
    /// One exact page occurrence with inherited geometry and resources.
    /// Its retained-memory reservation follows the value and its clones.
    Ready(Page),
    /// The page was already delivered.
    Complete,
    /// The lookup was cancelled before a page could be delivered.
    Cancelled,
}

#[derive(Debug)]
enum State {
    Active {
        document: Arc<Document>,
        cursor: Box<Cursor>,
    },
    Complete,
    Cancelled,
    Failed(Error),
}

/// Resolves a page ordinal without enumerating later subtrees or loading the
/// geometry/resources of preceding leaves. Counts never guess an ordinal: each
/// preceding leaf is structurally visited in Kids order. Repeated indirect
/// pages retain distinct occurrences; direct children use the shared repair path.
///
/// The active cursor owns its stack, source addresses, cumulative traversal
/// budget and pending reports. Missing dependencies preserve completed work and
/// an unchanged availability generation does no new work. Fixed controller
/// storage is separate from the document's page/resource retained-byte charges.
/// No network or filesystem I/O occurs. Complete enumeration remains available
/// through [`Document::pages`] and uses this same traversal owner.
#[derive(Debug)]
pub struct PageJob {
    state: State,
    work: usize,
    reported: Option<usize>,
    verified: Option<usize>,
}

impl PageJob {
    /// Starts lookup at zero-based `index` under the document's recovery policy.
    ///
    /// # Errors
    /// Returns lifecycle/catalog faults or `Limit` if the requested ordinal
    /// exceeds the page-count budget. Out-of-document ordinals are reported
    /// only after the structural traversal verifies the actual total.
    pub fn new(document: Arc<Document>, index: usize) -> ReadResult<Self> {
        document.check_source()?;
        if index >= document.limits().pages {
            return Err(Error::limit("page count budget exceeded").into());
        }
        let cursor = Box::new(Cursor::new(&document, Selection::Page(index))?);
        Ok(Self {
            state: State::Active { document, cursor },
            work: 0,
            reported: None,
            verified: None,
        })
    }

    /// Cumulative structural work, including retries and backlink reconstruction.
    pub fn work(&self) -> usize {
        self.work
    }

    /// The root's nonnegative direct Count, once read. This is an unverified
    /// document claim, and can disagree with the actual number of leaves.
    pub fn reported_count(&self) -> Option<usize> {
        self.reported
    }

    /// The actual leaf total, only after complete structural enumeration.
    /// A page returned before later subtrees are read leaves this unknown.
    pub fn verified_count(&self) -> Option<usize> {
        self.verified
    }

    /// Advances one structural unit or returns its missing dependency frontier.
    /// A ready page is delivered once; no partial page is returned.
    ///
    /// # Errors
    /// Returns `Unavailable` separately from terminal structure, policy, source
    /// and budget errors. Terminal errors repeat unchanged and release the
    /// cursor's provisional pages and document ownership.
    pub fn advance(&mut self) -> ReadResult<PageLoading> {
        let (document, cursor) = match &mut self.state {
            State::Active { document, cursor } => (document, cursor),
            State::Complete => return Ok(PageLoading::Complete),
            State::Cancelled => return Ok(PageLoading::Cancelled),
            State::Failed(error) => return Err(error.clone().into()),
        };
        let step = cursor.advance(document);
        self.work = cursor.work();
        self.reported = cursor.reported_count();
        if step.is_ok() {
            self.verified = cursor.verified_count();
        }
        let result = step.and_then(|complete| {
            if let Some(page) = cursor.take_page(document)? {
                Ok(PageLoading::Ready(page))
            } else if complete {
                Err(Error::invalid_argument("page index outside document").into())
            } else {
                Ok(PageLoading::Yielded)
            }
        });
        match &result {
            Ok(PageLoading::Ready(_)) => self.state = State::Complete,
            Err(ReadFault::Invalid(error)) => {
                cursor.publish(document);
                self.state = State::Failed(error.clone());
            }
            _ => {}
        }
        result
    }

    /// Cancels an active lookup and releases its demand and provisional storage.
    /// Already delivered pages and independent jobs are unaffected.
    pub fn cancel(&mut self) -> bool {
        if matches!(self.state, State::Active { .. }) {
            self.state = State::Cancelled;
            true
        } else {
            false
        }
    }
}
