//! Reported recovery of a complete declared single-page sequential file.
//!
//! PDF 7.4.7 forbids this packaging. T.88 D.1/D.4 and 7.3.2/7.4.9/7.4.11
//! identify its boundaries without changing any coded region or source offset.

use super::{Page, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result};

const MAGIC: [u8; 8] = [0x97, 0x4a, 0x42, 0x32, 0x0d, 0x0a, 0x1a, 0x0a];
const SEQUENTIAL_KNOWN_PAGES: u8 = 1;
const PAGE_INFORMATION: u8 = 48;
const END_PAGE: u8 = 49;
const END_FILE: u8 = 51;
const END_STRIPE: u8 = 50;
const REGION_KINDS: [u8; 12] = [4, 6, 7, 20, 22, 23, 36, 38, 39, 40, 42, 43];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stream {
    Globals,
    Page,
}

impl Stream {
    pub(super) const fn page(self) -> u32 {
        match self {
            Self::Globals => 0,
            Self::Page => 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    BeforePage,
    Page,
    EndPage,
    EndFile,
}

/// Original segment fields; no normalized byte buffer is constructed.
pub(super) struct Boundary {
    pub(super) kind: u8,
    pub(super) page: u32,
    pub(super) referrals: usize,
    pub(super) length: u32,
    pub(super) association: Position,
    pub(super) length_position: Position,
    pub(super) position: Position,
    pub(super) last: bool,
}

pub(super) struct Framing {
    stream: Stream,
    offset: usize,
    file: Option<Phase>,
    region_since_stripe: Option<Position>,
}

impl Framing {
    pub(super) fn new(data: &[u8], stream: Stream, diagnostics: &mut Diagnostics) -> Result<Self> {
        let mut framing = Self {
            stream,
            offset: 0,
            file: None,
            region_since_stripe: None,
        };
        if !data.starts_with(&MAGIC) {
            return Ok(framing);
        }
        if stream == Stream::Globals {
            return Err(forbidden());
        }
        let mut cursor = Cursor::new(data).with_message("truncated JBIG2 file header");
        cursor.take(MAGIC.len())?;
        if cursor.u8()? != SEQUENTIAL_KNOWN_PAGES || cursor.u32()? != 1 {
            return Err(forbidden());
        }
        diagnostics.repair_once(
            Diagnostic::new(
                repairs::FILE_WRAPPER,
                "JBIG2 file headers are forbidden in PDF",
            )
            .at(Position::Buffer(0)),
        )?;
        framing.offset = cursor.offset();
        framing.file = Some(Phase::BeforePage);
        Ok(framing)
    }

    pub(super) const fn offset(&self) -> usize {
        self.offset
    }

    /// Keep embedded-stream association validation and error precedence.
    pub(super) fn validate_page(&self, page: u32) -> Result<()> {
        if self.file.is_none() && page != self.stream.page() {
            return Err(Error::syntax("invalid JBIG2 embedded page association"));
        }
        Ok(())
    }

    /// Validates packaging before a terminal segment may be treated as metadata.
    pub(super) fn scope(
        &mut self,
        header: &Boundary,
        page: Option<&Page>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Stream>> {
        let Some(phase) = self.file else {
            return Ok(Some(self.stream));
        };
        if matches!(header.kind, END_PAGE | END_FILE) {
            self.end(header, phase, page, diagnostics)?;
            return Ok(None);
        }
        if header.kind == END_STRIPE {
            self.region_since_stripe = None;
        } else if REGION_KINDS.contains(&header.kind) {
            self.region_since_stripe = Some(header.position);
        }
        match (phase, header.kind, header.page) {
            (Phase::BeforePage, PAGE_INFORMATION, 1) if header.referrals == 0 => {
                self.file = Some(Phase::Page);
                Ok(Some(Stream::Page))
            }
            (Phase::BeforePage, kind, 0) if kind != PAGE_INFORMATION => Ok(Some(Stream::Globals)),
            (Phase::Page, kind, 1) if kind != PAGE_INFORMATION => Ok(Some(Stream::Page)),
            _ => Err(Error::syntax("invalid JBIG2 single-page file sequence").at(header.position)),
        }
    }

    fn end(
        &mut self,
        header: &Boundary,
        phase: Phase,
        page: Option<&Page>,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        if header.referrals != 0 {
            return Err(Error::syntax("JBIG2 terminal segment has referrals").at(header.position));
        }
        if header.length != 0 {
            return Err(Error::syntax("JBIG2 terminal segment has data").at(header.length_position));
        }
        if header.kind == END_PAGE {
            if phase != Phase::Page || header.page != 1 {
                return Err(
                    Error::syntax("invalid JBIG2 end-of-page boundary").at(header.association)
                );
            }
            if page.is_some_and(|page| page.unknown_height)
                && let Some(position) = self.region_since_stripe
            {
                return Err(Error::syntax(
                    "JBIG2 region between final stripe and end of page (T.88 7.4.9)",
                )
                .at(position));
            }
            self.file = Some(Phase::EndPage);
            return Ok(());
        }
        if phase != Phase::EndPage || !header.last || header.page > 1 {
            return Err(Error::syntax("invalid JBIG2 end-of-file boundary").at(header.association));
        }
        if header.page == 1 {
            // The sole page is already closed. An empty, unreferenced terminal
            // EOF contributes no pixels or resources; only its association is
            // nonconforming. This does not authorize any other page identity.
            diagnostics.repair_once(
                Diagnostic::new(
                    repairs::EOF_PAGE_ASSOCIATION,
                    "terminal JBIG2 EOF associated with the sole closed page (T.88 7.3.2)",
                )
                .at(header.association),
            )?;
        }
        self.file = Some(Phase::EndFile);
        Ok(())
    }

    pub(super) fn finish(&self, position: Position) -> Result<()> {
        if self.file.is_some_and(|phase| phase != Phase::EndFile) {
            return Err(Error::syntax("incomplete JBIG2 single-page file wrapper").at(position));
        }
        Ok(())
    }
}

fn forbidden() -> Error {
    Error::syntax("JBIG2 file headers are forbidden in PDF").at(Position::Buffer(0))
}
