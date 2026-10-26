//! Bounded source windows over the existing slice parser.

use super::{ByteLease, ByteRange, ByteSource, NeedData, ReadFault, ReadResult};
use pagura_core::convert::usize_to_u64;
use pagura_core::syntax::{
    InputEnd, Origin, ParseFault, ParsedWindow, Parser, WindowInput, WindowRead,
};
use pagura_core::{Error, Limits, Meter, Recovery, Result};
use std::sync::Arc;

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
    work: Meter,
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
            work: Meter::new(loading.work, "loading work budget exceeded"),
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
    pub const fn work(&self) -> usize {
        self.work.used()
    }

    /// Parses one owned value, growing its window when structure requires it.
    ///
    /// The callback must describe the same unit when retrying a wait. Keep its
    /// side effects provisional: it can run more than once. A completed
    /// `ParsedWindow::offset` is relative to this unit's starting slice;
    /// [`Self::offset`] then advances by that amount. Error/repair positions
    /// remain absolute file positions. Values cannot borrow a temporary lease.
    ///
    /// # Errors
    /// Returns exact missing intervals separately from terminal parser, source
    /// and budget failures. True EOF retains normal syntax errors. An unchanged
    /// wait does no parsing or source read, but still observes source closure
    /// and conflicting supply. Any terminal error permanently ends this reader.
    pub fn read<T>(
        &mut self,
        mut parse: impl FnMut(&mut Parser<'_, WindowInput>) -> std::result::Result<T, ParseFault>,
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
        parse: &mut impl FnMut(&mut Parser<'_, WindowInput>) -> std::result::Result<T, ParseFault>,
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
                .read_window(boundary, &mut *parse)?;
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
