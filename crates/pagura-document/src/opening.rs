//! Owned complete-document source admission and opening.

use crate::source::{ByteSource, LoadingLimits, NeedData, ReadFault, ReadResult, View};
use crate::{Document, OpenOptions, builder};
use pagura_core::{Error, Meter, Result};
use std::sync::Arc;

/// The result of advancing an owned document opener.
#[derive(Debug)]
#[non_exhaustive]
pub enum Opening {
    /// One complete, validated immutable document. Delivered exactly once.
    Ready(Arc<Document>),
    /// The ready document was already delivered.
    Complete,
    /// Opening was cancelled; no document can be published.
    Cancelled,
}

#[derive(Debug)]
enum State {
    Active(Box<Active>),
    Complete,
    Cancelled,
    Failed(Error),
}

#[derive(Debug)]
struct Active {
    source: Arc<dyn ByteSource>,
    options: OpenOptions,
    waiting: Option<NeedData>,
}

/// An owned opener with explicit complete-document admission.
///
/// Missing ranges retain the same registration and waiting state. Supplying
/// useful bytes permits another advance; repeated unchanged waits do no read
/// or parse work. Complete admission shares or explicitly assembles one source
/// lease, then uses the existing document builder, security and validation.
/// The document retains that lease and the source consistency provider.
///
/// This complete-document mode requests all input before parsing. It does not
/// promise requested-page readiness or a CPU yield inside complete validation.
/// Source assembly limits control materialization; the parser-window size
/// does not override that separate limit. Loading work charges one unit per
/// attempted admission and the admitted complete parse input once.
/// Fixed controller storage is excluded from source retained-byte accounting;
/// copied credentials are bounded by `Limits::string_bytes`.
#[derive(Debug)]
pub struct OpenJob {
    state: State,
    work: Meter,
}

impl OpenJob {
    /// Starts complete-document opening from an immutable registered source.
    /// The copied password is retained only while the opener is active.
    ///
    /// # Errors
    /// Returns source lifecycle errors or `Limit` when declared file size or
    /// password storage cannot be admitted. It performs no source byte read.
    pub fn new(
        source: Arc<dyn ByteSource>,
        options: &OpenOptions,
        loading: LoadingLimits,
    ) -> Result<Self> {
        source.check()?;
        crate::source::length(source.len(), &options.limits)?;
        if options.password.len() > options.limits.string_bytes {
            return Err(Error::limit("password byte budget exceeded"));
        }
        Ok(Self {
            state: State::Active(Box::new(Active {
                source,
                options: options.clone(),
                waiting: None,
            })),
            work: Meter::new(loading.work, "loading work budget exceeded"),
        })
    }

    /// Cumulative admitted work, retained after completion or cancellation.
    pub const fn work(&self) -> usize {
        self.work.used()
    }

    /// Advances opening or returns its unchanged missing-range frontier.
    ///
    /// # Errors
    /// Returns `Unavailable` until the complete source can be admitted.
    /// Parsing, consistency and budget errors are terminal; another advance
    /// returns the same failure without retrying or publishing a document.
    pub fn advance(&mut self) -> ReadResult<Opening> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(Opening::Complete),
            State::Cancelled => return Ok(Opening::Cancelled),
            State::Failed(error) => return Err(error.clone().into()),
        };
        match active.advance(&mut self.work) {
            Ok(document) => {
                self.state = State::Complete;
                Ok(Opening::Ready(Arc::new(document)))
            }
            Err(ReadFault::Invalid(error)) => {
                self.state = State::Failed(error.clone());
                Err(error.into())
            }
            Err(error) => Err(error),
        }
    }

    /// Cancels active opening and releases its source, frontier and password.
    /// Does not close the shared source or change a completed/failed outcome.
    /// Returns whether this call changed an active opener.
    pub fn cancel(&mut self) -> bool {
        if matches!(self.state, State::Active(_)) {
            self.state = State::Cancelled;
            true
        } else {
            false
        }
    }
}

impl Active {
    fn advance(&mut self, work: &mut Meter) -> ReadResult<Document> {
        self.source.check()?;
        if let Some(missing) = &self.waiting
            && missing.generation() == self.source.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        work.charge(1)?;
        let lease = match self.source.materialize() {
            Ok(lease) => lease,
            Err(ReadFault::Unavailable(missing)) => {
                self.waiting = Some(missing.clone());
                return Err(ReadFault::Unavailable(missing));
            }
            Err(error) => return Err(error),
        };
        work.charge(lease.bytes().len())?;
        let bytes = View::admitted(Arc::clone(&self.source), lease)?;
        let document = builder::open(bytes, &self.options)?;
        document.bytes.check()?;
        Ok(document)
    }
}
