//! Owned complete-document source admission and opening.

use crate::source::{
    ByteSource, LoadingLimits, LoadingWork, NeedData, ReadFault, ReadResult, View,
};
use crate::{Document, OpenOptions, builder, xref};
use pagura_core::{Diagnostics, Error, Recovery, Result};
use std::sync::Arc;

/// The result of advancing an owned document opener.
#[derive(Debug)]
#[non_exhaustive]
pub enum Opening {
    /// One bootstrap unit completed; advancing again can make CPU progress.
    Yielded,
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
    loading: LoadingLimits,
    phase: Phase,
}

#[derive(Debug)]
enum Phase {
    Index(Box<xref::Loader>),
    Admitting(Box<Admission>),
    Complete,
}

#[derive(Debug)]
struct Admission {
    primary: xref::Primary,
    waiting: Option<NeedData>,
}

/// An owned opener with explicit complete-document admission.
///
/// Missing ranges retain the same registration and waiting state. Supplying
/// useful bytes permits another advance; repeated unchanged waits do no read
/// or parse work. Header, final tail and xref sections complete independently,
/// preserving the shared newest-wins revision chain between waits. The final
/// complete-document admission shares or explicitly assembles one source lease,
/// then uses the existing document builder, security and global validation.
/// The document retains the lease and source consistency provider.
///
/// This mode yields between bootstrap units and admits all input before final
/// validation. It does not promise requested-page readiness or a CPU yield
/// inside complete validation.
/// Source assembly limits control materialization; the parser-window size
/// does not override that separate limit. Loading work charges one unit per
/// attempted read/admission, all parser windows and the final complete input.
/// Fixed controller storage is excluded from source retained-byte accounting;
/// copied credentials are bounded by `Limits::string_bytes`.
#[derive(Debug)]
pub struct OpenJob {
    state: State,
    work: Arc<LoadingWork>,
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
        let work = Arc::new(LoadingWork::new(loading.work));
        let loader = xref::Loader::new(
            Arc::clone(&source),
            options.limits,
            loading,
            options.recovery,
            Arc::clone(&work),
        )?;
        Ok(Self {
            state: State::Active(Box::new(Active {
                source,
                options: options.clone(),
                loading,
                phase: Phase::Index(Box::new(loader)),
            })),
            work,
        })
    }

    /// Cumulative admitted work, retained after completion or cancellation.
    pub fn work(&self) -> usize {
        self.work.used()
    }

    /// Advances opening or returns its unchanged missing-range frontier.
    ///
    /// # Errors
    /// Returns exact unavailable bootstrap/admission ranges.
    /// Parsing, consistency and budget errors are terminal; another advance
    /// returns the same failure without retrying or publishing a document.
    pub fn advance(&mut self) -> ReadResult<Opening> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(Opening::Complete),
            State::Cancelled => return Ok(Opening::Cancelled),
            State::Failed(error) => return Err(error.clone().into()),
        };
        match active.advance(&self.work) {
            Ok(Opening::Ready(document)) => {
                self.state = State::Complete;
                Ok(Opening::Ready(document))
            }
            Ok(Opening::Complete) => {
                self.state = State::Complete;
                Ok(Opening::Complete)
            }
            Err(ReadFault::Invalid(error)) => {
                self.state = State::Failed(error.clone());
                Err(error.into())
            }
            result => result,
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
    fn advance(&mut self, work: &Arc<LoadingWork>) -> ReadResult<Opening> {
        match &mut self.phase {
            Phase::Index(loader) => {
                let primary = match loader.advance() {
                    Ok(()) => loader.take_completed(),
                    Err(ReadFault::Invalid(error))
                        if self.options.recovery == Recovery::Repair
                            && xref::structural(&error) =>
                    {
                        Some(loader.take_failure(error.clone()).ok_or(error)?)
                    }
                    Err(error) => return Err(error),
                };
                if let Some(primary) = primary {
                    self.phase = Phase::Admitting(Box::new(Admission {
                        primary,
                        waiting: None,
                    }));
                }
                Ok(Opening::Yielded)
            }
            Phase::Admitting(_) => self.admit(work),
            Phase::Complete => Ok(Opening::Complete),
        }
    }

    fn admit(&mut self, work: &Arc<LoadingWork>) -> ReadResult<Opening> {
        let Phase::Admitting(admission) = &mut self.phase else {
            return Ok(Opening::Yielded);
        };
        self.source.check()?;
        if let Some(missing) = &admission.waiting
            && missing.generation() == self.source.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        admission.waiting = None;
        work.charge(1)?;
        let lease = match self.source.materialize() {
            Ok(lease) => lease,
            Err(ReadFault::Unavailable(missing)) => {
                admission.waiting = Some(missing.clone());
                return Err(ReadFault::Unavailable(missing));
            }
            Err(error) => return Err(error),
        };
        work.charge(lease.bytes().len())?;
        let bytes = View::admitted(
            Arc::clone(&self.source),
            lease,
            self.options.limits,
            self.loading,
            Arc::clone(work),
        )?;
        self.finish(bytes)
    }

    fn finish(&mut self, bytes: View) -> ReadResult<Opening> {
        debug_assert!(matches!(self.phase, Phase::Admitting(_)));
        let admission = match std::mem::replace(&mut self.phase, Phase::Complete) {
            Phase::Admitting(admission) => admission,
            phase => {
                self.phase = phase;
                return Ok(Opening::Yielded);
            }
        };
        let primary = admission.primary;
        let mut diagnostics = Diagnostics::new(self.options.recovery);
        diagnostics.absorb(primary.header_diagnostics)?;
        let lease = bytes.materialize()?;
        let index = primary.index.and_then(|(table, reports)| {
            xref::finish(lease.bytes(), primary.header, &self.options.limits, table)
                .map(|table| (table, reports))
        });
        let document = builder::from_primary(bytes, &self.options, diagnostics, index)?;
        document.bytes.check()?;
        Ok(Opening::Ready(Arc::new(document)))
    }
}
