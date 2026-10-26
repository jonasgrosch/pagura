//! Owned source opening with explicit validation scope and atomic publication.

use crate::source::{
    ByteSource, LoadingLimits, LoadingWork, NeedData, ReadFault, ReadResult, View,
};
use crate::{Document, OpenOptions, Validation, builder, xref};
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Recovery, Result};
use std::sync::Arc;

/// The result of advancing an owned document opener.
#[derive(Debug)]
#[non_exhaustive]
pub enum Opening {
    /// One bootstrap/construction unit completed; another advance can progress.
    Yielded,
    /// One immutable document, with its applied [`Document::validation`] policy.
    /// Delivered exactly once; later reads may need ranges in requested scope.
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
    bytes: View,
    options: OpenOptions,
    validation: Validation,
    phase: Phase,
    header_diagnostics: Option<Vec<Diagnostic>>,
    rebuilt: bool,
    waiting: Option<NeedData>,
}

#[derive(Debug)]
enum Phase {
    Index(Box<xref::Loader>),
    Indexed(Box<Indexed>),
    Build(Box<builder::Build>),
    Rebuild(Position),
    Complete,
}

#[derive(Debug)]
struct Indexed {
    header: usize,
    table: xref::XrefTable,
    diagnostics: Vec<Diagnostic>,
    admitted: bool,
}

/// An owned opener for one immutable registered source and validation policy.
///
/// The authoritative newest-wins index, security and catalog are retained
/// between waits. Completed dependency caches survive ordinary missing ranges;
/// no partial document is published. Complete scope explicitly materializes
/// all bytes and validates indexed identities. Requested scope loads only the
/// opening dependencies; unrelated object bodies/headers remain deferred.
/// Structural reconstruction, when permitted, explicitly requires all bytes.
///
/// Source assembly limits control materialization; parser-window size does not
/// override that separate limit. Loading work charges before reads and syntax
/// and remains shared with the ready document. Fixed controller storage is
/// excluded from source retained-byte accounting; copied credentials are bounded
/// by `Limits::string_bytes` and retained only while opening is active.
#[derive(Debug)]
pub struct OpenJob {
    state: State,
    work: Arc<LoadingWork>,
}

impl OpenJob {
    /// Starts opening without reading source bytes.
    ///
    /// # Errors
    /// Returns lifecycle or admission errors for the declared size, credentials
    /// or loading configuration. Missing bytes are reported by [`Self::advance`].
    pub fn new(
        source: Arc<dyn ByteSource>,
        options: &OpenOptions,
        loading: LoadingLimits,
        validation: Validation,
    ) -> Result<Self> {
        let work = Arc::new(LoadingWork::new(loading.work));
        let bytes = View::registered(source, options.limits, loading, Arc::clone(&work))?;
        Ok(Self {
            state: State::Active(Box::new(Active::new(bytes, options, validation)?)),
            work,
        })
    }

    /// Cumulative admitted work, shared with the ready document and retained
    /// after opening completes or is cancelled.
    pub fn work(&self) -> usize {
        self.work.used()
    }

    /// Advances one opening unit or returns its unchanged missing-range frontier.
    ///
    /// # Errors
    /// Returns unavailable dependency ranges separately from terminal parsing,
    /// consistency and budget failures. Terminal failures repeat unchanged;
    /// cancelled or failed input can never publish a document.
    pub fn advance(&mut self) -> ReadResult<Opening> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(Opening::Complete),
            State::Cancelled => return Ok(Opening::Cancelled),
            State::Failed(error) => return Err(error.clone().into()),
        };
        match active.advance() {
            Ok(Some(document)) => {
                self.state = State::Complete;
                Ok(Opening::Ready(Arc::new(document)))
            }
            Ok(None) => Ok(Opening::Yielded),
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

/// Complete-buffer convenience and reauthentication drive this same owner.
/// Source waits retain their typed meaning; callers decide whether to own a job.
pub(crate) fn open(
    bytes: View,
    options: &OpenOptions,
    validation: Validation,
) -> ReadResult<Document> {
    let mut active = Active::new(bytes, options, validation)?;
    loop {
        if let Some(document) = active.advance()? {
            return Ok(document);
        }
    }
}

impl Active {
    fn new(bytes: View, options: &OpenOptions, validation: Validation) -> Result<Self> {
        bytes.check()?;
        if options.password.len() > options.limits.string_bytes {
            return Err(Error::limit("password byte budget exceeded"));
        }
        let loader = xref::Loader::new(
            Arc::new(bytes.clone()),
            options.limits,
            bytes.loading(),
            options.recovery,
            bytes.work(),
        )?;
        Ok(Self {
            bytes,
            options: options.clone(),
            validation,
            phase: Phase::Index(Box::new(loader)),
            header_diagnostics: None,
            rebuilt: false,
            waiting: None,
        })
    }

    fn advance(&mut self) -> ReadResult<Option<Document>> {
        self.bytes.check()?;
        if let Some(missing) = &self.waiting
            && missing.generation() == self.bytes.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        match self.step() {
            Err(ReadFault::Unavailable(missing)) => {
                self.waiting = Some(missing.clone());
                Err(ReadFault::Unavailable(missing))
            }
            Err(ReadFault::Invalid(error))
                if !self.rebuilt
                    && self.header_diagnostics.is_some()
                    && self.options.recovery == Recovery::Repair
                    && xref::structural(&error) =>
            {
                self.rebuilt = true;
                self.phase = Phase::Rebuild(error.position);
                Ok(None)
            }
            result => result,
        }
    }

    fn step(&mut self) -> ReadResult<Option<Document>> {
        match &mut self.phase {
            Phase::Index(loader) => {
                let primary = match loader.advance() {
                    Ok(()) => loader.take_completed(),
                    Err(ReadFault::Invalid(error)) => {
                        Some(loader.take_failure(error.clone()).ok_or(error)?)
                    }
                    Err(error) => return Err(error),
                };
                if let Some(primary) = primary {
                    self.header_diagnostics = Some(primary.header_diagnostics);
                    let (table, diagnostics) = primary.index?;
                    self.phase = Phase::Indexed(Box::new(Indexed {
                        header: primary.header,
                        table,
                        diagnostics,
                        admitted: false,
                    }));
                }
                Ok(None)
            }
            Phase::Indexed(_) => self.construct(),
            Phase::Build(build) => {
                let ready = build.advance(&self.options.password)?;
                if ready.is_some() {
                    self.phase = Phase::Complete;
                }
                Ok(ready)
            }
            Phase::Rebuild(position) => {
                let position = *position;
                let work = self.bytes.work();
                work.charge(1)?;
                let lease = self.bytes.materialize()?;
                work.charge(lease.bytes().len())?;
                let mut diagnostics = Diagnostics::new(self.options.recovery);
                diagnostics.absorb(self.header_reports())?;
                let table = xref::rebuild::read(
                    lease.bytes(),
                    &self.options.limits,
                    &mut diagnostics,
                    position,
                )?;
                self.phase = Phase::Build(Box::new(builder::Build::new(
                    builder::DocumentBuilder::new(
                        self.bytes.clone(),
                        &self.options,
                        table,
                        diagnostics.take(),
                        self.validation,
                    )?,
                )));
                Ok(None)
            }
            Phase::Complete => Ok(None),
        }
    }

    fn header_reports(&self) -> impl Iterator<Item = Diagnostic> + '_ {
        self.header_diagnostics.iter().flatten().cloned()
    }

    fn construct(&mut self) -> ReadResult<Option<Document>> {
        let Phase::Indexed(index) = &mut self.phase else {
            return Ok(None);
        };
        if self.validation == Validation::CompleteDocument && !index.admitted {
            let work = self.bytes.work();
            work.charge(1)?;
            let lease = self.bytes.materialize()?;
            work.charge(lease.bytes().len())?;
            index.admitted = true;
        }
        xref::finish(
            &self.bytes,
            index.header,
            &self.options.limits,
            self.validation,
            &mut index.table,
        )?;
        let previous = std::mem::replace(&mut self.phase, Phase::Complete);
        debug_assert!(matches!(previous, Phase::Indexed(_)));
        if let Phase::Indexed(index) = previous {
            let reports = self.header_reports().chain(index.diagnostics).collect();
            self.phase = Phase::Build(Box::new(builder::Build::new(
                builder::DocumentBuilder::new(
                    self.bytes.clone(),
                    &self.options,
                    index.table,
                    reports,
                    self.validation,
                )?,
            )));
        }
        Ok(None)
    }
}
