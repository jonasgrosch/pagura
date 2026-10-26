//! One bounded payload writer for source waits and owned-job progress.

use crate::protocol::{JobEventCode, OutputCode, Payload, ResponseKind, ValidationCode};
use crate::wire::Writer;
use pagura::source::NeedData;
use pagura::{BandLease, DeviceRect, Page, RenderOptions, Result, Validation, png};

#[derive(Debug)]
pub(super) enum Event {
    Yielded,
    Opened {
        document: u32,
        validation: Validation,
    },
    Page {
        page: Page,
        reported: Option<usize>,
        verified: Option<usize>,
    },
    Prepared {
        output: OutputCode,
        page: DeviceRect,
        region: DeviceRect,
    },
    Output(Output),
    Complete,
    Cancelled,
    Backpressured,
}

/// Keeps the canonical producer slot leased until its explicit acknowledgement.
#[derive(Debug)]
pub(super) enum Output {
    Band(BandLease),
    Png(png::ChunkLease),
}

pub(super) fn need_data(missing: &NeedData, cap: usize) -> Result<Payload> {
    let mut out = Writer::new(cap);
    out.u64(missing.identity().registration())?;
    out.push(&missing.identity().version())?;
    out.u64(missing.generation())?;
    out.count(missing.ranges().len())?;
    for range in missing.ranges() {
        out.u64(range.start())?;
        out.u64(range.end())?;
    }
    Ok(Payload::new(ResponseKind::NeedData, out))
}

impl Event {
    pub(super) fn encode(&self, job: u32, sequence: u64, cap: usize) -> Result<Payload> {
        let mut out = Writer::new(cap);
        out.u32(job)?;
        out.u8(self.code().code())?;
        match self {
            Self::Opened {
                document,
                validation,
            } => {
                out.u32(*document)?;
                out.u8(match validation {
                    Validation::CompleteDocument => ValidationCode::CompleteDocument,
                    Validation::RequestedOperation => ValidationCode::RequestedOperation,
                    _ => return Err(pagura::Error::invalid_argument("unknown validation scope")),
                }
                .code())?;
            }
            Self::Page {
                page,
                reported,
                verified,
            } => {
                out.count(page.index())?;
                optional_count(&mut out, *reported)?;
                optional_count(&mut out, *verified)?;
                let crop = page.crop_box();
                out.f64s(&[crop.x0(), crop.y0(), crop.x1(), crop.y1()])?;
                out.u16(page.rotation().degrees())?;
                out.f64(page.user_unit())?;
                let (width, height) = pagura::device_size(page, &RenderOptions::default())?;
                out.u32(width)?;
                out.u32(height)?;
            }
            Self::Prepared {
                output,
                page,
                region,
            } => {
                out.u8(output.code())?;
                rectangle(&mut out, *page)?;
                rectangle(&mut out, *region)?;
            }
            Self::Output(output) => {
                out.u64(sequence)?;
                match output {
                    Output::Band(band) => {
                        rectangle(&mut out, band.page())?;
                        rectangle(&mut out, band.region())?;
                        out.u32(band.rows().start)?;
                        out.u32(band.rows().end)?;
                        out.count(band.stride())?;
                        out.count(band.premultiplied_rgba().len())?;
                        out.push(band.premultiplied_rgba())?;
                    }
                    Output::Png(chunk) => {
                        out.count(chunk.bytes().len())?;
                        out.push(chunk.bytes())?;
                    }
                }
            }
            Self::Yielded | Self::Complete | Self::Cancelled | Self::Backpressured => {}
        }
        Ok(Payload::new(ResponseKind::JobProgress, out))
    }

    const fn code(&self) -> JobEventCode {
        match self {
            Self::Yielded => JobEventCode::Yielded,
            Self::Opened { .. } => JobEventCode::Opened,
            Self::Page { .. } => JobEventCode::Page,
            Self::Prepared { .. } => JobEventCode::Prepared,
            Self::Output(Output::Band(_)) => JobEventCode::Band,
            Self::Output(Output::Png(_)) => JobEventCode::Png,
            Self::Complete => JobEventCode::Complete,
            Self::Cancelled => JobEventCode::Cancelled,
            Self::Backpressured => JobEventCode::Backpressured,
        }
    }
}

fn rectangle(out: &mut Writer, rect: DeviceRect) -> Result<()> {
    for value in [rect.x0, rect.y0, rect.x1, rect.y1] {
        out.u32(value)?;
    }
    Ok(())
}

fn optional_count(out: &mut Writer, count: Option<usize>) -> Result<()> {
    out.flag(count.is_some())?;
    if let Some(count) = count {
        out.count(count)?;
    }
    Ok(())
}
