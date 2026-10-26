//! One-row conversion from the owned renderer to the existing PNG encoder.

use super::{ColorType, Encoder, EncodingLimits, EncodingOutput, Level};
use crate::{BandLease, BandOutput, Error, RenderSession, Result, Rgba8};
use pagura_core::Meter;

#[derive(Debug)]
struct Feeding {
    row: Vec<u8>,
    band: Option<BandLease>,
    offset: usize,
    next_row: u32,
}

#[derive(Debug)]
enum State {
    Feeding(Feeding),
    Complete,
    Cancelled,
    Failed(Error),
}

/// An owned prepared render encoded as PNG without allocating a full surface.
///
/// Preparation, render slots, one conversion row and all encoder storage are
/// admitted before output. The adapter uses the raster's existing straight-alpha
/// conversion and the same incremental PNG encoder as complete-buffer output.
#[derive(Debug)]
pub struct RenderEncoder {
    render: RenderSession,
    encoder: Encoder,
    state: State,
}

impl RenderEncoder {
    /// Admits PNG output for an owned render that has not produced any band.
    /// Its requested region determines the PNG dimensions, including for tiles.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an already advanced or terminal render,
    /// and the allocation, conservative output and work errors of [`Encoder::new`].
    /// The render is consumed on success or failure; no output has been published.
    pub fn new(render: RenderSession, level: Level, mut limits: EncodingLimits) -> Result<Self> {
        let region = render.region();
        if render.next_row() != Some(region.y0) {
            return Err(Error::invalid_argument("PNG requires an unstarted render"));
        }
        let stride = usize::try_from(region.width())
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(Error::limit("PNG conversion row size overflow"))?;
        let mut row = Vec::new();
        let mut bytes = Meter::new(limits.retained_bytes, "PNG conversion row budget exceeded");
        bytes.reserve_capacity(&mut row, stride)?;
        limits.retained_bytes -= bytes.used();
        let encoder = Encoder::new(
            region.width(),
            region.height(),
            ColorType::Rgba,
            level,
            limits,
        )?;
        Ok(Self {
            render,
            encoder,
            state: State::Feeding(Feeding {
                row,
                band: None,
                offset: 0,
                next_row: region.y0,
            }),
        })
    }

    /// Encoding storage, the conversion row and leased encoded output.
    /// Renderer scene/scratch/output reservations remain separate paint domains.
    pub fn encoding_bytes(&self) -> usize {
        let row = match &self.state {
            State::Feeding(feeding) => feeding.row.capacity(),
            _ => 0,
        };
        self.encoder.retained_bytes().saturating_add(row)
    }

    /// Advances at most one render band, conversion row or compression block.
    /// Output waits perform no renderer or encoder work. Input is already prepared,
    /// so this adapter yields progress instead of exposing pixel-input demand.
    ///
    /// # Errors
    /// Returns a terminal encoder failure or an inconsistent band/row sequence.
    /// Cancellation or failure cannot later become successful completion.
    pub fn advance(&mut self) -> Result<EncodingOutput> {
        match &self.state {
            State::Complete => return Ok(EncodingOutput::Complete),
            State::Cancelled => return Ok(EncodingOutput::Cancelled),
            State::Failed(error) => return Err(error.clone()),
            State::Feeding(_) => {}
        }
        let result = self.step();
        if let Err(error) = &result {
            self.render.cancel();
            self.encoder.cancel();
            self.state = State::Failed(error.clone());
        }
        result
    }

    fn step(&mut self) -> Result<EncodingOutput> {
        match self.encoder.advance()? {
            EncodingOutput::NeedInput => self.supply_row(),
            EncodingOutput::Complete => {
                let State::Feeding(feeding) = &self.state else {
                    return Err(Error::invalid_argument("PNG render state is terminal"));
                };
                if feeding.next_row != self.render.region().y1 || self.render.next_row().is_some() {
                    return Err(Error::invalid_argument(
                        "PNG finished before the render region",
                    ));
                }
                self.state = State::Complete;
                Ok(EncodingOutput::Complete)
            }
            other => Ok(other),
        }
    }

    fn supply_row(&mut self) -> Result<EncodingOutput> {
        let State::Feeding(feeding) = &mut self.state else {
            return Err(Error::invalid_argument("PNG render state is terminal"));
        };
        let Some(band) = &feeding.band else {
            return match self.render.advance() {
                BandOutput::Band(band) => {
                    if band.rows().start != feeding.next_row
                        || band.region() != self.render.region()
                    {
                        return Err(Error::invalid_argument("PNG render band sequence mismatch"));
                    }
                    feeding.band = Some(band);
                    feeding.offset = 0;
                    Ok(EncodingOutput::Yielded)
                }
                BandOutput::Backpressured => Ok(EncodingOutput::Backpressured),
                BandOutput::Complete | BandOutput::Cancelled => Err(Error::invalid_argument(
                    "PNG input ended before its declared rows",
                )),
                _ => Err(Error::invalid_argument("unknown render output state")),
            };
        };
        let end = feeding
            .offset
            .checked_add(band.stride())
            .ok_or(Error::limit("PNG row offset overflow"))?;
        let source = band
            .premultiplied_rgba()
            .get(feeding.offset..end)
            .ok_or(Error::invalid_argument("PNG render row is truncated"))?;
        feeding.row.clear();
        for &pixel in source.as_chunks::<4>().0 {
            let color = Rgba8::from_premultiplied(pixel);
            feeding
                .row
                .extend_from_slice(&[color.r, color.g, color.b, color.a]);
        }
        if self.encoder.write(&feeding.row)? != feeding.row.len() {
            return Err(Error::invalid_argument(
                "PNG did not accept a complete render row",
            ));
        }
        feeding.next_row = feeding
            .next_row
            .checked_add(1)
            .ok_or(Error::limit("PNG row count overflow"))?;
        feeding.offset = end;
        if end == band.premultiplied_rgba().len() {
            feeding.band = None;
        }
        Ok(EncodingOutput::Yielded)
    }

    /// Cancels both encoding and rendering, releasing the held band and row.
    /// Encoded leases remain valid and charged until their final owner drops them.
    pub fn cancel(&mut self) {
        if matches!(self.state, State::Feeding(_)) {
            self.render.cancel();
            self.encoder.cancel();
            self.state = State::Cancelled;
        }
    }
}
