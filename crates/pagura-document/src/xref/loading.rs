//! Owned range bootstrap using the complete reader's grammar and revision chain.

use super::{XrefTable, boundary, chain, parse_section, section_header, stream};
use crate::source::{ByteSource, LoadingLimits, LoadingWork, ReadFault, ReadResult, Window};
use pagura_core::convert::usize_to_u64;
use pagura_core::{Diagnostic, Diagnostics, Error, ErrorKind, Limits, Position, Recovery, Result};
use std::sync::Arc;

#[derive(Debug)]
enum Phase {
    Header,
    Tail,
    Physical(usize),
    Shifted(usize),
    Sections(Box<chain::Chain>),
    Complete,
}

enum Next {
    Stay,
    Seek(usize),
    Restart(usize),
}

/// A completed primary attempt; its header reports survive reconstruction.
#[derive(Debug)]
pub(crate) struct Primary {
    pub(crate) header: usize,
    pub(crate) header_diagnostics: Vec<Diagnostic>,
    pub(crate) index: Result<(XrefTable, Vec<Diagnostic>)>,
}

#[derive(Debug)]
pub(crate) struct Loader {
    source: Arc<dyn ByteSource>,
    limits: Limits,
    loading: LoadingLimits,
    work: Arc<LoadingWork>,
    recovery: Recovery,
    length: usize,
    window: Window,
    phase: Phase,
    header: Option<(usize, Vec<Diagnostic>)>,
    diagnostics: Diagnostics,
}

impl Loader {
    pub(crate) fn new(
        source: Arc<dyn ByteSource>,
        limits: Limits,
        loading: LoadingLimits,
        recovery: Recovery,
        work: Arc<LoadingWork>,
    ) -> Result<Self> {
        let length = crate::source::length(source.len(), &limits)?;
        let window =
            Window::new_with_work(Arc::clone(&source), 0, limits, loading, Arc::clone(&work))?
                .with_recovery(recovery);
        Ok(Self {
            source,
            limits,
            loading,
            work,
            recovery,
            length,
            window,
            phase: Phase::Header,
            header: None,
            diagnostics: Diagnostics::new(recovery),
        })
    }

    /// One completed grammar unit per advance. Every window charges the same
    /// owner before reading/parsing, including incomplete and failed probes.
    pub(crate) fn advance(&mut self) -> ReadResult<()> {
        let next = self.step()?;
        if let Next::Seek(offset) | Next::Restart(offset) = next
            && offset > self.length
        {
            // This offset came from PDF structure, not a source API caller.
            return Err(Error::syntax("offset outside input")
                .at(Position::File(offset))
                .into());
        }
        match next {
            Next::Stay => {}
            Next::Seek(offset) => self.window.seek(usize_to_u64(offset))?,
            Next::Restart(offset) => {
                self.window = Window::new_with_work(
                    Arc::clone(&self.source),
                    usize_to_u64(offset),
                    self.limits,
                    self.loading,
                    Arc::clone(&self.work),
                )?
                .with_recovery(self.recovery);
            }
        }
        Ok(())
    }

    pub(crate) fn take_completed(&mut self) -> Option<Primary> {
        match std::mem::replace(&mut self.phase, Phase::Complete) {
            Phase::Sections(chain) if chain.request().is_none() => {
                debug_assert!(self.header.is_some());
                let (header, header_diagnostics) = self.header.take()?;
                Some(Primary {
                    header,
                    header_diagnostics,
                    index: Ok((chain.into_table(), self.diagnostics.take())),
                })
            }
            phase => {
                self.phase = phase;
                None
            }
        }
    }

    /// Discards a failed primary, never resuming its partially merged chain.
    /// A failure before header completion does not permit reconstruction.
    pub(crate) fn take_failure(&mut self, error: Error) -> Option<Primary> {
        let (header, header_diagnostics) = self.header.take()?;
        self.phase = Phase::Complete;
        Some(Primary {
            header,
            header_diagnostics,
            index: Err(error),
        })
    }

    fn step(&mut self) -> ReadResult<Next> {
        match &mut self.phase {
            Phase::Header => self.read_header(),
            Phase::Tail => self.read_tail(),
            Phase::Physical(start) => {
                let start = *start;
                self.probe(start, Probe::Physical)
            }
            Phase::Shifted(start) => {
                let start = *start;
                self.probe(start, Probe::Shifted)
            }
            Phase::Sections(chain) => read_section(
                &mut self.window,
                chain,
                &self.limits,
                self.recovery,
                self.length,
                &mut self.diagnostics,
            ),
            Phase::Complete => Ok(Next::Stay),
        }
    }

    fn read_header(&mut self) -> ReadResult<Next> {
        let parsed = self.window.read(|parser| {
            let mut diagnostics = Diagnostics::new(self.recovery);
            let header = boundary::parse_header(parser, &mut diagnostics)?;
            diagnostics.absorb(parser.take_diagnostics())?;
            Ok((header, diagnostics.take()))
        })?;
        self.header = Some(parsed.value);
        self.phase = Phase::Tail;
        Ok(Next::Seek(
            self.length.saturating_sub(boundary::TAIL_WINDOW),
        ))
    }

    fn read_tail(&mut self) -> ReadResult<Next> {
        let origin = self.length.saturating_sub(boundary::TAIL_WINDOW);
        let length = self.length - origin;
        let parsed = self.window.read(|parser| {
            parser.has_bytes(length)?;
            let mut diagnostics = Diagnostics::new(self.recovery);
            let start = boundary::startxref(
                parser.data(),
                origin,
                self.length,
                &self.limits,
                &mut diagnostics,
            )?
            .ok_or(
                Error::syntax("missing startxref near the end of the file")
                    .at(Position::File(self.length)),
            )?;
            parser.seek(length)?;
            Ok((start, diagnostics.take()))
        })?;
        let (start, diagnostics) = parsed.value;
        self.diagnostics.absorb(diagnostics)?;
        if self.header.as_ref().is_some_and(|(offset, _)| *offset != 0) {
            self.phase = Phase::Physical(start);
        } else {
            self.phase = Phase::Sections(Box::new(chain::Chain::new(start, 0, &self.limits)?));
        }
        Ok(Next::Seek(start))
    }

    fn probe(&mut self, start: usize, mode: Probe) -> ReadResult<Next> {
        let read = self.window.read(section_header);
        let (matches, restart) = match read {
            Ok(parsed) => (parsed.value, false),
            Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Syntax => (false, true),
            Err(error) => return Err(error),
        };
        let header = self.header.as_ref().map_or(0, |(offset, _)| *offset);
        let displaced = start
            .checked_add(header)
            .filter(|&offset| offset < self.length);
        let offset = if mode == Probe::Physical
            && !matches
            && let Some(offset) = displaced
        {
            self.phase = Phase::Shifted(start);
            offset
        } else {
            let shift = if mode == Probe::Shifted && matches {
                header
            } else {
                0
            };
            let offset = start
                .checked_add(shift)
                .ok_or(Error::syntax("xref offset overflow"))?;
            self.phase = Phase::Sections(Box::new(chain::Chain::new(offset, shift, &self.limits)?));
            offset
        };
        Ok(if restart {
            Next::Restart(offset)
        } else {
            Next::Seek(offset)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    Physical,
    Shifted,
}

fn read_section(
    window: &mut Window,
    chain: &mut chain::Chain,
    limits: &Limits,
    recovery: Recovery,
    length: usize,
    diagnostics: &mut Diagnostics,
) -> ReadResult<Next> {
    let Some(request) = chain.request() else {
        return Ok(Next::Stay);
    };
    let limits = Limits {
        objects: request.objects,
        ..*limits
    };
    let parsed = window.read(|parser| {
        let mut diagnostics = Diagnostics::new(recovery);
        let section = match request.kind {
            chain::Kind::Section => parse_section(parser, length, &limits, &mut diagnostics),
            chain::Kind::Stream => stream::parse(parser, length, &limits, &mut diagnostics),
        }?;
        Ok((section, diagnostics.take()))
    })?;
    let (mut section, reports) = parsed.value;
    section.end = section
        .end
        .checked_add(request.offset)
        .ok_or(Error::syntax("xref offset overflow"))?;
    chain.accept(section)?;
    diagnostics.absorb(reports)?;
    Ok(chain
        .request()
        .map_or(Next::Stay, |request| Next::Seek(request.offset)))
}
