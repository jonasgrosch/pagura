//! Bounded source windows over the existing slice parser.

use super::{ByteLease, ByteRange, ByteSource, NeedData, ReadFault, ReadResult};
use pagura_core::convert::usize_to_u64;
use pagura_core::syntax::{
    InputEnd, Origin, ParseFault, ParsedWindow, Parser, WindowInput, WindowRead,
};
use pagura_core::{Error, Limits, Meter, Recovery, Result};
use std::sync::{Arc, Mutex, PoisonError};

/// Loading work separate from PDF syntax, source residency and rendering.
///
/// Public fields intentionally support functional record update. Window and
/// retry work is cumulative for the reader; starting another read does not
/// reset it. Source leases/frontiers remain charged to [`super::SourceLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadingLimits {
    /// Maximum contiguous bytes inspected by one parser attempt.
    pub window_bytes: usize,
    /// Cumulative parser-window bytes plus one unit per attempted source read.
    pub work: usize,
}

impl Default for LoadingLimits {
    fn default() -> Self {
        Self {
            window_bytes: 16 * 1024 * 1024,
            work: 1024 * 1024 * 1024,
        }
    }
}

/// One cumulative loading-work owner shared across bootstrap and later readers.
/// A lock covers only the charge arithmetic, never source access or parsing.
#[derive(Debug)]
pub(crate) struct LoadingWork(Mutex<Meter>);

impl LoadingWork {
    pub(crate) fn new(limit: usize) -> Self {
        Self(Mutex::new(Meter::new(
            limit,
            "loading work budget exceeded",
        )))
    }

    pub(crate) fn charge(&self, count: usize) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .charge(count)
    }

    pub(crate) fn used(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).used()
    }
}

/// One source cursor with bounded parser windows and resumable input waits.
///
/// `read` retries only the current syntactic unit. It retains the needed
/// extent across missing-input returns, suppresses repeated work at the same
/// availability generation and advances only after successful completion.
/// An available prefix can complete a value even when later bytes are absent.
/// Terminal errors stay terminal. No source lock or lease is held while the
/// reader waits; the missing-range record retains its own source-budget charge.
///
/// This is a syntax adapter, not an opened or validated PDF. It uses the same
/// parser for complete and sparse sources and publishes no partial value or
/// provisional repair. Its fixed inline controller is outside retained-byte
/// accounting; source allocations and parser values retain their own budgets.
#[derive(Debug)]
pub struct Window {
    source: Arc<dyn ByteSource>,
    limits: Limits,
    recovery: Recovery,
    at: usize,
    length: usize,
    cap: usize,
    work: Arc<LoadingWork>,
    waiting: Option<NeedData>,
    terminal: Option<Error>,
}

const INITIAL_BYTES: usize = 256;

impl Window {
    /// Starts a source cursor at absolute file `offset`.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an offset beyond the declared source,
    /// `Limit` for target/file/window admission, or a terminal source error.
    pub fn new(
        source: Arc<dyn ByteSource>,
        offset: u64,
        limits: Limits,
        loading: LoadingLimits,
    ) -> Result<Self> {
        Self::new_with_work(
            source,
            offset,
            limits,
            loading,
            Arc::new(LoadingWork::new(loading.work)),
        )
    }

    pub(crate) fn new_with_work(
        source: Arc<dyn ByteSource>,
        offset: u64,
        limits: Limits,
        loading: LoadingLimits,
        work: Arc<LoadingWork>,
    ) -> Result<Self> {
        source.check()?;
        let length = super::length(source.len(), &limits)?;
        let at = ByteRange::new(offset, offset)?.offsets(source.len())?.start;
        if loading.window_bytes == 0 && at < length {
            return Err(Error::limit("parser window byte budget exceeded"));
        }
        Ok(Self {
            source,
            limits,
            recovery: Recovery::Strict,
            at,
            length: INITIAL_BYTES.min(loading.window_bytes).min(length - at),
            cap: loading.window_bytes,
            work,
            waiting: None,
            terminal: None,
        })
    }

    /// Selects the parser's reported-repair policy before reading.
    #[must_use]
    pub fn with_recovery(mut self, recovery: Recovery) -> Self {
        self.recovery = recovery;
        self
    }

    /// The absolute source offset of the next unfinished syntactic unit.
    pub fn offset(&self) -> u64 {
        usize_to_u64(self.at)
    }

    /// Cumulative loading work, including discarded incomplete attempts.
    pub fn work(&self) -> usize {
        self.work.used()
    }

    /// Starts another unit at an absolute file offset, abandoning a pending
    /// unit while retaining this reader's cumulative work charge. No source
    /// bytes are fetched and no previously terminal failure is cleared.
    ///
    /// # Errors
    /// Returns the prior terminal failure or a source lifecycle error; an
    /// out-of-file offset returns `InvalidArgument`. Failed repositioning
    /// leaves the cursor and any pending unit unchanged.
    pub fn seek(&mut self, offset: u64) -> Result<()> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        self.source.check()?;
        let length = super::length(self.source.len(), &self.limits)?;
        let at = ByteRange::new(offset, offset)?
            .offsets(self.source.len())?
            .start;
        if self.cap == 0 && at < length {
            return Err(Error::limit("parser window byte budget exceeded"));
        }
        self.at = at;
        self.length = INITIAL_BYTES.min(self.cap).min(length - at);
        self.waiting = None;
        Ok(())
    }

    /// Parses one owned value, growing its window when structure requires it.
    ///
    /// The callback must describe the same unit when retrying a wait. Keep its
    /// side effects provisional: it can run more than once. A completed
    /// `ParsedWindow::offset` is relative to this unit's starting slice;
    /// [`Self::offset`] then advances by that amount. Error/repair positions
    /// remain absolute file positions. Values cannot borrow a temporary lease.
    /// A callback may propagate a dependency read from the same source through
    /// `ParseFault::Deferred(NeedData)`; that frontier does not grow this slice
    /// or advance the cursor. Another source identity is an argument error.
    ///
    /// # Errors
    /// Returns exact missing intervals separately from terminal parser, source
    /// and budget failures. True EOF retains normal syntax errors. An unchanged
    /// wait does no parsing or source read, but still observes source closure
    /// and conflicting supply. Any terminal error permanently ends this reader.
    pub fn read<T>(
        &mut self,
        mut parse: impl FnMut(
            &mut Parser<'_, WindowInput<NeedData>>,
        ) -> std::result::Result<T, ParseFault<NeedData>>,
    ) -> ReadResult<ParsedWindow<T>> {
        if let Some(error) = &self.terminal {
            return Err(error.clone().into());
        }
        let result = self.attempt(&mut parse);
        match &result {
            Err(ReadFault::Invalid(error)) => {
                self.waiting = None;
                self.terminal = Some(error.clone());
            }
            Err(ReadFault::Unavailable(missing)) => self.waiting = Some(missing.clone()),
            Ok(_) => self.waiting = None,
        }
        result
    }

    fn attempt<T>(
        &mut self,
        parse: &mut impl FnMut(
            &mut Parser<'_, WindowInput<NeedData>>,
        ) -> std::result::Result<T, ParseFault<NeedData>>,
    ) -> ReadResult<ParsedWindow<T>> {
        self.source.check()?;
        if let Some(missing) = &self.waiting
            && missing.generation() == self.source.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        let source_end = super::length(self.source.len(), &self.limits)?;
        loop {
            let end = self.at + self.length;
            let range = ByteRange::new(usize_to_u64(self.at), usize_to_u64(end))?;
            let (lease, missing) = self.lease(range)?;
            self.work.charge(lease.bytes().len())?;
            let boundary = if lease.range().end() == usize_to_u64(source_end) {
                InputEnd::Complete
            } else {
                InputEnd::Extendable
            };
            let read = Parser::new(lease.bytes(), Origin::File, self.limits)
                .with_origin_offset(self.at)?
                .with_recovery(self.recovery)
                .read_window_with_control(boundary, &mut *parse)?;
            match read {
                WindowRead::Complete(parsed) => {
                    self.at += parsed.offset;
                    self.length = INITIAL_BYTES.min(self.cap).min(source_end - self.at);
                    return Ok(parsed);
                }
                WindowRead::Incomplete { minimum_length } => {
                    if let Some(missing) = missing {
                        return Err(ReadFault::Unavailable(missing));
                    }
                    let available = self.cap.min(source_end - self.at);
                    if minimum_length > available {
                        return Err(Error::limit("parser window byte budget exceeded").into());
                    }
                    self.length = self
                        .length
                        .saturating_mul(2)
                        .max(minimum_length)
                        .min(available);
                }
                WindowRead::Deferred(missing) => {
                    if missing.identity() != self.source.identity() {
                        return Err(Error::invalid_argument(
                            "window callback dependency belongs to another source",
                        )
                        .into());
                    }
                    return Err(ReadFault::Unavailable(missing));
                }
                _ => return Err(Error::invalid_argument("unknown parser window state").into()),
            }
        }
    }

    fn lease(&mut self, range: ByteRange) -> ReadResult<(ByteLease, Option<NeedData>)> {
        self.work.charge(1)?;
        match self.source.read_exact(range) {
            Ok(lease) => Ok((lease, None)),
            Err(ReadFault::Unavailable(missing)) => {
                let prefix = missing
                    .ranges()
                    .first()
                    .map(|gap| gap.start())
                    .filter(|&end| end > range.start());
                let Some(end) = prefix else {
                    return Err(ReadFault::Unavailable(missing));
                };
                self.work.charge(1)?;
                let lease = self
                    .source
                    .read_exact(ByteRange::new(range.start(), end)?)?;
                Ok((lease, Some(missing)))
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadingLimits, LoadingWork, Window};
    use crate::source::{CompleteBytes, ReadFault, SourceIdentity, SourceLimits};
    use pagura_core::{ErrorKind, Limits};
    use std::{cell::Cell, sync::Arc};

    #[test]
    fn dependent_windows_share_one_precharged_budget_without_holding_its_lock() {
        let limits = Limits::default();
        let source = Arc::new(
            CompleteBytes::new(
                SourceIdentity::new(81, [23; 32]),
                Arc::from(b"1 2 ".as_slice()),
                &limits,
                &SourceLimits::default(),
            )
            .unwrap(),
        );
        // The first 4-byte window costs 1 + 4; its 2-byte dependency costs
        // 1 + 2. No allowance remains for a third read or parser callback.
        let work = Arc::new(LoadingWork::new(8));
        let make = |offset| {
            Window::new_with_work(
                source.clone(),
                offset,
                limits,
                LoadingLimits::default(),
                Arc::clone(&work),
            )
        };
        let mut first = make(0).unwrap();
        let mut dependency = make(2).unwrap();
        let value = first
            .read(|parser| {
                let value = parser.unsigned()?;
                let other = dependency.read(|parser| parser.unsigned())?;
                Ok((value, other.value))
            })
            .unwrap();
        assert_eq!(value.value, (1, 2));
        assert_eq!(first.work(), 8);
        assert_eq!(dependency.work(), 8);
        let mut another = make(0).unwrap();
        let called = Cell::new(false);
        for _ in 0..3 {
            let result = another.read(|parser| {
                called.set(true);
                parser.unsigned()
            });
            assert!(
                matches!(result, Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit)
            );
            assert_eq!(another.work(), 8);
            assert!(!called.get());
        }
    }
}
