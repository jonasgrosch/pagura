//! Host lifetime and acknowledgement state around the canonical owned jobs.

use super::super::documents::{Documents, Ownership};
use super::events::{Event, Output};
use crate::protocol::{Field, OutputCode, Payload};
use pagura::{
    BandOutput, Error, OpenJob, Opening, PageJob, PageLoading, ReadFault, ReadResult, RenderJob,
    RenderLoading, RenderSession, Reservation, Result, png,
};
use std::sync::Arc;

#[derive(Debug)]
struct OpeningTask {
    job: OpenJob,
    admission: Reservation,
}

#[derive(Debug)]
struct PageTask {
    job: PageJob,
    _ownership: Arc<Ownership>,
}

#[derive(Debug)]
struct RenderOwnership {
    _document: Arc<Ownership>,
    _admission: Reservation,
}

#[derive(Debug)]
struct LoadingTask {
    job: RenderJob,
    output: OutputCode,
    encoding: png::EncodingLimits,
    ownership: RenderOwnership,
}

#[derive(Debug)]
enum Producer {
    Bands(RenderSession),
    Png(png::RenderEncoder),
}

#[derive(Debug)]
struct ProducingTask {
    producer: Producer,
    _ownership: RenderOwnership,
}

#[derive(Debug)]
enum Phase {
    Open(Box<OpeningTask>),
    Page(Box<PageTask>),
    Loading(Box<LoadingTask>),
    Producing(Box<ProducingTask>),
    Complete,
    Cancelled,
    Failed(Error),
}

#[derive(Debug)]
pub(super) struct Job {
    phase: Phase,
    pending: Option<Output>,
    sequence: u64,
}

impl Job {
    pub(super) fn open(job: OpenJob, admission: Reservation) -> Self {
        Self::new(Phase::Open(Box::new(OpeningTask { job, admission })))
    }
    pub(super) fn page(job: PageJob, ownership: Arc<Ownership>) -> Self {
        Self::new(Phase::Page(Box::new(PageTask {
            job,
            _ownership: ownership,
        })))
    }
    pub(super) fn render(
        job: RenderJob,
        output: OutputCode,
        encoding: png::EncodingLimits,
        document: Arc<Ownership>,
        admission: Reservation,
    ) -> Self {
        Self::new(Phase::Loading(Box::new(LoadingTask {
            job,
            output,
            encoding,
            ownership: RenderOwnership {
                _document: document,
                _admission: admission,
            },
        })))
    }
    const fn new(phase: Phase) -> Self {
        Self {
            phase,
            pending: None,
            sequence: 0,
        }
    }

    pub(super) fn advance(
        &mut self,
        handle: u32,
        documents: &mut Documents,
        cap: usize,
    ) -> ReadResult<Payload> {
        if self.pending.is_some() {
            return Ok(Event::Backpressured.encode(handle, self.sequence, cap)?);
        }
        let phase = std::mem::replace(&mut self.phase, Phase::Complete);
        let (phase, result) = phase.advance(documents);
        self.phase = phase;
        let result = result.and_then(|event| {
            let sequence = if matches!(event, Event::Output(_)) {
                self.sequence
                    .checked_add(1)
                    .ok_or(Error::limit("job output sequence overflow"))?
            } else {
                self.sequence
            };
            let payload = event.encode(handle, sequence, cap)?;
            if let Event::Output(output) = event {
                self.sequence = sequence;
                self.pending = Some(output);
            }
            Ok(payload)
        });
        if let Err(ReadFault::Invalid(error)) = &result {
            self.phase = Phase::Failed(error.clone());
            self.pending = None;
        }
        result
    }

    pub(super) fn acknowledge(&mut self, sequence: Field<u64>) -> Result<()> {
        if self.pending.is_none() || sequence.get() != self.sequence {
            return Err(
                Error::invalid_argument("no matching outstanding job output")
                    .at(sequence.position()),
            );
        }
        self.pending = None;
        Ok(())
    }

    pub(super) fn cancel(&mut self) -> bool {
        if matches!(
            self.phase,
            Phase::Complete | Phase::Cancelled | Phase::Failed(_)
        ) {
            return false;
        }
        self.phase = Phase::Cancelled;
        self.pending = None;
        true
    }
}

impl Phase {
    fn advance(self, documents: &mut Documents) -> (Self, ReadResult<Event>) {
        match self {
            Self::Open(mut task) => match task.job.advance() {
                Ok(Opening::Ready(document)) => {
                    let validation = document.validation();
                    let result = documents
                        .insert_progressive(document, task.admission)
                        .map(|document| Event::Opened {
                            document,
                            validation,
                        })
                        .map_err(ReadFault::from);
                    (Self::Complete, result)
                }
                Ok(Opening::Yielded) => (Self::Open(task), Ok(Event::Yielded)),
                Ok(Opening::Complete) => (Self::Complete, Ok(Event::Complete)),
                Ok(_) => (Self::Cancelled, Ok(Event::Cancelled)),
                Err(error) => (Self::Open(task), Err(error)),
            },
            Self::Page(mut task) => match task.job.advance() {
                Ok(PageLoading::Ready(page)) => {
                    let event = Event::Page {
                        page,
                        reported: task.job.reported_count(),
                        verified: task.job.verified_count(),
                    };
                    (Self::Complete, Ok(event))
                }
                Ok(PageLoading::Yielded) => (Self::Page(task), Ok(Event::Yielded)),
                Ok(PageLoading::Complete) => (Self::Complete, Ok(Event::Complete)),
                Ok(_) => (Self::Cancelled, Ok(Event::Cancelled)),
                Err(error) => (Self::Page(task), Err(error)),
            },
            Self::Loading(mut task) => match task.job.advance() {
                Ok(RenderLoading::Ready(render)) => {
                    let event = Event::Prepared {
                        output: task.output,
                        page: render.page(),
                        region: render.region(),
                    };
                    let producer = match task.output {
                        OutputCode::Bands => Ok(Producer::Bands(render)),
                        OutputCode::Png => {
                            png::RenderEncoder::new(render, png::Level::Default, task.encoding)
                                .map(Producer::Png)
                        }
                    };
                    match producer {
                        Ok(producer) => (
                            Self::Producing(Box::new(ProducingTask {
                                producer,
                                _ownership: task.ownership,
                            })),
                            Ok(event),
                        ),
                        Err(error) => (Self::Failed(error.clone()), Err(error.into())),
                    }
                }
                Ok(RenderLoading::Yielded) => (Self::Loading(task), Ok(Event::Yielded)),
                Ok(RenderLoading::Complete) => (Self::Complete, Ok(Event::Complete)),
                Ok(_) => (Self::Cancelled, Ok(Event::Cancelled)),
                Err(error) => (Self::Loading(task), Err(error)),
            },
            Self::Producing(mut task) => match task.producer.advance() {
                Ok(Event::Complete) => (Self::Complete, Ok(Event::Complete)),
                Ok(Event::Cancelled) => (Self::Cancelled, Ok(Event::Cancelled)),
                result => (Self::Producing(task), result.map_err(ReadFault::from)),
            },
            Self::Complete => (Self::Complete, Ok(Event::Complete)),
            Self::Cancelled => (Self::Cancelled, Ok(Event::Cancelled)),
            Self::Failed(error) => (Self::Failed(error.clone()), Err(error.into())),
        }
    }
}

impl Producer {
    fn advance(&mut self) -> Result<Event> {
        Ok(match self {
            Self::Bands(render) => match render.advance() {
                BandOutput::Band(band) => Event::Output(Output::Band(band)),
                BandOutput::Backpressured => Event::Backpressured,
                BandOutput::Complete => Event::Complete,
                _ => Event::Cancelled,
            },
            Self::Png(encoder) => match encoder.advance()? {
                png::EncodingOutput::Chunk(chunk) => Event::Output(Output::Png(chunk)),
                png::EncodingOutput::Yielded => Event::Yielded,
                png::EncodingOutput::Backpressured => Event::Backpressured,
                png::EncodingOutput::Complete => Event::Complete,
                _ => Event::Cancelled,
            },
        })
    }
}
