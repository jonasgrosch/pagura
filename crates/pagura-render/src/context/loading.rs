//! Source/page/scene preparation hands off to the canonical owned band session.

use super::RenderContext;
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Meter};
use pagura_document::source::{NeedData, ReadFault, ReadResult};
use pagura_document::{Page, PageJob, PageLoading};
use pagura_interpret::Scene;
use pagura_paint::{OutputLimits, RenderSession};
use std::sync::Arc;

/// Bounds repeated scene attempts while dependencies arrive.
/// Public fields intentionally support functional record update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderJobLimits {
    /// Maximum scene-recording calls, charged before each attempt. Each call
    /// retains the ordinary interpreter/font/colour budgets. Unchanged waits
    /// consume no attempt; cache hits still count as a recording call.
    pub scene_attempts: usize,
}

impl Default for RenderJobLimits {
    fn default() -> Self {
        Self { scene_attempts: 64 }
    }
}

/// Progress of page lookup, atomic recording and render preflight.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderLoading {
    /// One loading/preparation unit completed; no pixels have been published.
    Yielded,
    /// An unstarted owned session. It needs no more source reads and can produce
    /// final bands or feed a streaming encoder after the context/source closes.
    Ready(RenderSession),
    /// The prepared session was already delivered.
    Complete,
    /// Cancellation is terminal; no session can be delivered.
    Cancelled,
}

#[derive(Debug)]
enum Phase {
    Page(PageJob),
    Record(Page),
    Prepare(Arc<Scene>),
}

#[derive(Debug)]
struct Active {
    context: Arc<RenderContext>,
    options: RenderOptions,
    tile: Option<Tile>,
    output: OutputLimits,
    phase: Phase,
    waiting: Option<NeedData>,
}

#[derive(Debug)]
enum State {
    Active(Box<Active>),
    Complete,
    Cancelled,
    Failed(Error),
}

/// Owns source-dependent work until a complete preflighted render can begin.
///
/// The shared page cursor resolves metadata; the existing context records and
/// caches one atomic scene; its existing render-session constructor prepares
/// every mark and admits scratch/output storage. There is no second interpreter
/// or executor. A missing dependency drops the unfinished recording device while
/// keeping completed source/font/scene dependencies in their existing caches.
///
/// Recording currently retries an unfinished scene after a changed source
/// generation. This is bounded by [`RenderJobLimits`], not claimed to resume an
/// interpreter instruction. Source loading work remains cumulative in Document;
/// per-attempt interpreter work remains subject to the document's normal limits.
#[derive(Debug)]
pub struct RenderJob {
    state: State,
    attempts: Meter,
}

impl RenderJob {
    /// Starts loading `index` for the whole page or the requested device tile.
    ///
    /// # Errors
    /// As [`PageJob::new`]. Rendering options and output storage are validated
    /// by the canonical preflight before any session or output is published.
    pub fn new(
        context: Arc<RenderContext>,
        index: usize,
        options: RenderOptions,
        tile: Option<Tile>,
        output: OutputLimits,
        limits: RenderJobLimits,
    ) -> ReadResult<Self> {
        let page = PageJob::new(Arc::clone(&context.document), index)?;
        Ok(Self {
            state: State::Active(Box::new(Active {
                context,
                options,
                tile,
                output,
                phase: Phase::Page(page),
                waiting: None,
            })),
            attempts: Meter::new(limits.scene_attempts, "scene attempt budget exceeded"),
        })
    }

    /// Scene-recording calls admitted so far, including incomplete attempts.
    pub fn scene_attempts(&self) -> usize {
        self.attempts.used()
    }

    /// Advances one page-tree unit, scene attempt or complete render preflight.
    /// No context/source/store lock is held across the return.
    ///
    /// # Errors
    /// Returns exact source waits separately from terminal page, interpretation,
    /// preparation and budget failures. Failed and cancelled jobs never deliver
    /// a prepared session; a terminal failure repeats unchanged.
    pub fn advance(&mut self) -> ReadResult<RenderLoading> {
        let active = match &mut self.state {
            State::Active(active) => active,
            State::Complete => return Ok(RenderLoading::Complete),
            State::Cancelled => return Ok(RenderLoading::Cancelled),
            State::Failed(error) => return Err(error.clone().into()),
        };
        let result = active.advance(&mut self.attempts);
        match &result {
            Ok(RenderLoading::Ready(_)) => self.state = State::Complete,
            Ok(RenderLoading::Cancelled) => self.state = State::Cancelled,
            Err(ReadFault::Invalid(error)) => self.state = State::Failed(error.clone()),
            _ => {}
        }
        result
    }

    /// Cancels active loading and releases the context, page and pending demand.
    /// Already delivered sessions and unrelated jobs retain their own ownership.
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
    fn advance(&mut self, attempts: &mut Meter) -> ReadResult<RenderLoading> {
        self.context.document.check_source()?;
        if let Some(missing) = &self.waiting
            && !self.context.document.can_retry(missing)?
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        let result = self.step(attempts);
        if let Err(ReadFault::Unavailable(missing)) = &result {
            self.waiting = Some(missing.clone());
        }
        result
    }

    fn step(&mut self, attempts: &mut Meter) -> ReadResult<RenderLoading> {
        match &mut self.phase {
            Phase::Page(job) => match job.advance()? {
                PageLoading::Ready(page) => self.phase = Phase::Record(page),
                PageLoading::Yielded => {}
                // A child without a pending page cannot produce successful output.
                _ => return Ok(RenderLoading::Cancelled),
            },
            Phase::Record(page) => {
                attempts.charge(1)?;
                let scene = self.context.scene(page)?;
                self.phase = Phase::Prepare(scene);
            }
            Phase::Prepare(scene) => {
                let session =
                    self.context
                        .render_session(scene, &self.options, self.tile, self.output)?;
                self.context.document.check_source()?;
                return Ok(RenderLoading::Ready(session));
            }
        }
        Ok(RenderLoading::Yielded)
    }
}
