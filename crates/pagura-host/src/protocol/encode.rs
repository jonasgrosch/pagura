//! Response encoding. Tile pixels and reading payloads are never copied
//! into the frame: they follow the encoded fields as a borrowed tail.

use super::{DiagnosticCode, ErrorCategory, Pixels, Response, TileStatus, layout};
use crate::wire::Writer;
use pagura::{Diagnostic, Error, Result};
use std::fmt;
use std::io::{self, Write};

/// One encoded response frame: its length prefix and fields, then a borrowed
/// tail of pixels or reading payload.
pub struct Encoded<'a> {
    pub(super) head: Vec<u8>,
    pub(super) tail: &'a [u8],
}

impl Encoded<'_> {
    /// Writes the whole frame, without copying the tail.
    ///
    /// # Errors
    /// Returns the output's I/O errors.
    pub fn write_to(&self, output: &mut impl Write) -> io::Result<()> {
        output.write_all(&self.head)?;
        output.write_all(self.tail)
    }

    /// The whole frame in one buffer.
    pub fn to_vec(&self) -> Vec<u8> {
        [self.head.as_slice(), self.tail].concat()
    }
}

impl fmt::Debug for Encoded<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Encoded")
            .field("head", &self.head)
            .field("tail_bytes", &self.tail.len())
            .finish()
    }
}

/// Encode a response including its length prefix, checking the cap before
/// any allocation.
///
/// # Errors
/// Returns `Limit` if the body exceeds `max_body`, the wire's `u32` length,
/// or available memory. A tile whose byte count does not match its
/// dimensions is `InvalidArgument`, as is an error or diagnostic kind added
/// to pagura-core that no protocol version encodes yet.
pub fn encode(id: u32, response: &Response, max_body: usize) -> Result<Encoded<'_>> {
    let cap = max_body.min(layout::MAX_BODY_BYTES);
    let mut out = Writer::new(cap.saturating_add(layout::LENGTH_BYTES));
    let length = out.slot::<{ layout::LENGTH_BYTES }>()?;
    out.u8(response.kind().code())?;
    out.u32(id)?;
    let tail = payload(&mut out, response)?;
    let body = (out.len() - layout::LENGTH_BYTES)
        .checked_add(tail.len())
        .filter(|&body| body <= cap)
        .and_then(|body| u32::try_from(body).ok())
        .ok_or(Error::limit("response frame budget exceeded"))?;
    out.fill(length, body.to_le_bytes());
    Ok(Encoded {
        head: out.finish(),
        tail,
    })
}

/// Writes the fields of `response` and returns the bytes that follow them.
fn payload<'a>(out: &mut Writer, response: &'a Response) -> Result<&'a [u8]> {
    match response {
        Response::Hello(version) => out.u32(version.code())?,
        Response::Open { document, pages } => {
            out.u32(*document)?;
            out.u32(*pages)?;
        }
        Response::PageInfo {
            crop,
            rotation,
            user_unit,
            size: (width, height),
        } => {
            out.f64s(crop)?;
            out.u16(*rotation)?;
            out.f64(*user_unit)?;
            out.u32(*width)?;
            out.u32(*height)?;
        }
        Response::Tile(pixels) => return tile(out, pixels),
        Response::Skipped(generation) => {
            out.u8(TileStatus::Skipped.code())?;
            out.u32(*generation)?;
        }
        Response::GenerationTile { generation, pixels } => {
            out.u8(TileStatus::Rendered.code())?;
            out.u32(*generation)?;
            return tile(out, pixels);
        }
        Response::Generation(generation) => out.u32(*generation)?,
        Response::Stats(stats) => {
            out.u64(stats.source_bytes)?;
            out.u64(stats.cache_bytes)?;
            out.u32(stats.documents)?;
            out.u32(stats.lists)?;
        }
        Response::Reading(response) => return Ok(response.payload()),
        Response::Diagnostics(items) => {
            let repairs = || items.iter().filter(|item| item.repair().is_some());
            out.count(repairs().count())?;
            for item in repairs() {
                diagnostic(out, item)?;
            }
        }
        Response::DiagnosticDetails(items) => {
            out.count(items.len())?;
            for item in items {
                out.u8(DiagnosticCode::try_from(&item.kind)?.code())?;
                diagnostic(out, item)?;
            }
        }
        Response::Close => (),
        Response::Error(error) => {
            out.u8(ErrorCategory::try_from(&error.kind)?.code())?;
            out.text(error.feature().map_or("", pagura::Feature::as_str))?;
            out.text(error.repair.map_or("", pagura::Repair::as_str))?;
            out.text(error.message)?;
            out.context(error.object, error.position)?;
        }
    }
    Ok(&[])
}

/// `text tag | text message | context`.
fn diagnostic(out: &mut Writer, item: &Diagnostic) -> Result<()> {
    out.text(item.tag())?;
    out.text(item.message)?;
    out.context(item.object, item.position)
}

/// `u32 width | u32 height`, then exactly `width * height * 4` bytes.
fn tile<'a>(out: &mut Writer, pixels: &'a Pixels) -> Result<&'a [u8]> {
    let expected = u64::from(pixels.width) * u64::from(pixels.height);
    if pixels.width == 0
        || pixels.height == 0
        || expected.checked_mul(4) != u64::try_from(pixels.rgba.len()).ok()
    {
        return Err(Error::invalid_argument(
            "tile byte count does not match dimensions",
        ));
    }
    out.u32(pixels.width)?;
    out.u32(pixels.height)?;
    Ok(&pixels.rgba)
}
