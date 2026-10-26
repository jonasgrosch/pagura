//! Source and job handles extend the existing session and document registry.

mod events;
mod jobs;
mod sources;

use super::documents::Documents;
use crate::progressive::Request;
use crate::protocol::{Field, OutputCode, PageRef, Payload, PngBackground, ResponseKind, layout};
use crate::wire::Writer;
use crate::{ProgressiveLimits, SessionConfig};
use jobs::Job;
use pagura::source::NeedData;
use pagura::{
    Error, MemoryBudget, OpenJob, OpenOptions, OverprintPreview, ReadResult, Recovery, RenderJob,
    RenderOptions, Result, Rgba8, Tile, Validation,
};
use sources::Sources;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct Registry {
    config: SessionConfig,
    limits: ProgressiveLimits,
    sources: Sources,
    jobs: BTreeMap<u32, Job>,
    next: u32,
    active: Arc<MemoryBudget>,
}

impl Registry {
    pub(super) fn new(config: &SessionConfig, source: Arc<MemoryBudget>) -> Self {
        let limits = config.progressive_limits();
        Self {
            config: *config,
            limits,
            sources: Sources::new(source),
            jobs: BTreeMap::new(),
            next: 1,
            active: MemoryBudget::new(limits.active_bytes, "session active job admission exceeded"),
        }
    }

    fn cap(&self) -> usize {
        self.config.session.reading_response_bytes.min(
            self.config
                .session
                .max_frame_bytes
                .saturating_sub(layout::HEADER_BYTES),
        )
    }

    pub(super) fn need_data(&self, missing: &NeedData) -> Result<Payload> {
        events::need_data(missing, self.cap())
    }

    pub(super) fn execute(
        &mut self,
        request: Request<'_>,
        documents: &mut Documents,
    ) -> ReadResult<Payload> {
        let cap = self.cap();
        // Every non-output result fits this fixed envelope. Check the envelope
        // before changing handles; variable payloads use their own bounded writer.
        if cap < 128 {
            return Err(Error::limit("progressive response budget is too small").into());
        }
        let mut out = Writer::new(cap);
        let kind = request.kind().response();
        match request {
            Request::SourceCreate { length, version } => {
                out.u32(self.sources.create(
                    length,
                    version,
                    &self.config.engine,
                    &self.limits,
                )?)?;
            }
            Request::SourceSupply {
                source,
                version,
                offset,
                bytes,
            } => {
                let supply = self
                    .sources
                    .supply(source, version, offset, bytes, &self.limits)?;
                out.u64(pagura::convert::usize_to_u64(supply.added_bytes()))?;
                out.u64(supply.generation())?;
            }
            Request::SourceFinish(handle) => {
                self.sources.get(handle)?.finish()?;
                out.u32(handle.get())?;
            }
            Request::SourceClose(handle) => {
                self.sources.close(handle)?;
                out.u32(handle.get())?;
            }
            Request::Open {
                source,
                validation,
                recovery,
                password,
            } => {
                out.u32(self.open(source, validation, recovery, password)?)?;
            }
            Request::Page(page) => {
                self.admit_job()?;
                let (job, ownership) = documents.page_job(page)?;
                out.u32(self.insert(Job::page(job, ownership)))?;
            }
            Request::Render {
                page,
                scale,
                background,
                output,
                tile,
            } => {
                self.admit_job()?;
                if !(scale.get().is_finite() && scale.get() > 0.0) {
                    return Err(Error::invalid_argument("scale must be positive and finite")
                        .at(scale.position())
                        .into());
                }
                let options = RenderOptions {
                    scale: scale.get(),
                    background: match background.get() {
                        PngBackground::White => Rgba8::WHITE,
                        PngBackground::Transparent => Rgba8::TRANSPARENT,
                    },
                    overprint: OverprintPreview::Auto,
                };
                out.u32(self.render(
                    documents,
                    page,
                    options,
                    output.get(),
                    tile.map(Field::get),
                )?)?;
            }
            Request::Advance(handle) => {
                return self.job(handle)?.advance(handle.get(), documents, cap);
            }
            Request::Acknowledge { job, sequence } => {
                self.job(job)?.acknowledge(sequence)?;
                out.u32(job.get())?;
                out.u64(sequence.get())?;
            }
            Request::Cancel(handle) => {
                let changed = self.job(handle)?.cancel();
                out.u32(handle.get())?;
                out.flag(changed)?;
            }
            Request::Close(handle) => {
                self.jobs
                    .remove(&handle.get())
                    .ok_or_else(|| unknown(handle))?;
                out.u32(handle.get())?;
            }
            Request::Stats => return Ok(self.stats()?),
        }
        Ok(Payload::new(kind, out))
    }

    fn stats(&self) -> Result<Payload> {
        let mut out = Writer::new(self.cap());
        out.count(self.sources.count())?;
        out.count(self.jobs.len())?;
        out.u64(pagura::convert::usize_to_u64(self.sources.admitted_bytes()))?;
        out.u64(pagura::convert::usize_to_u64(self.active.used()))?;
        out.u64(pagura::convert::usize_to_u64(
            self.sources.registered_bytes(),
        ))?;
        Ok(Payload::new(ResponseKind::JobStats, out))
    }

    fn open(
        &mut self,
        source: Field<u32>,
        validation: Validation,
        recovery: Recovery,
        password: &[u8],
    ) -> Result<u32> {
        self.admit_job()?;
        if password.len() > self.config.engine.string_bytes {
            return Err(Error::limit("password byte budget exceeded"));
        }
        let admission = self.active.reserve(
            self.limits
                .opening_bytes(&self.config.engine, self.config.session.store_bytes)?,
        )?;
        let source = self.sources.get(source)?;
        let options = OpenOptions {
            limits: self.config.engine,
            recovery,
            password: password.to_vec(),
        };
        let job = OpenJob::new(source, &options, self.limits.loading, validation)?;
        Ok(self.insert(Job::open(job, admission)))
    }

    fn render(
        &mut self,
        documents: &Documents,
        page: PageRef,
        options: RenderOptions,
        output: OutputCode,
        tile: Option<Tile>,
    ) -> ReadResult<u32> {
        // A producer slot cannot exceed its complete storage ceiling. Admit
        // either canonical output representation before any pixels or PNG.
        let bytes = self
            .limits
            .output
            .bytes
            .max(self.limits.encoding.retained_bytes);
        if bytes > self.cap().saturating_sub(64) {
            return Err(Error::limit("job output cannot fit a response frame").into());
        }
        let admission = self
            .active
            .reserve(self.limits.render_bytes(&self.config.engine)?)?;
        let (context, ownership) = documents.pin(page.document)?;
        let job = RenderJob::new(
            context,
            page.page.index()?,
            options,
            tile,
            self.limits.output,
            self.limits.render,
        )?;
        Ok(self.insert(Job::render(
            job,
            output,
            self.limits.encoding,
            ownership,
            admission,
        )))
    }

    fn admit_job(&self) -> Result<()> {
        if self.jobs.len() >= self.limits.jobs || self.next == u32::MAX {
            return Err(Error::limit("session job handle budget exceeded"));
        }
        Ok(())
    }

    fn insert(&mut self, job: Job) -> u32 {
        let handle = self.next;
        self.next += 1;
        self.jobs.insert(handle, job);
        handle
    }

    fn job(&mut self, handle: Field<u32>) -> Result<&mut Job> {
        self.jobs
            .get_mut(&handle.get())
            .ok_or_else(|| unknown(handle))
    }
}

fn unknown(handle: Field<u32>) -> Error {
    Error::invalid_argument("unknown job handle").at(handle.position())
}
