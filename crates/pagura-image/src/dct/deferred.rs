//! Baseline JPEG frames kept in their native component planes
//! ([`Layout::Native`](crate::Layout::Native)).
//!
//! Blocks are decoded from a bounded unstuffed copy of each entropy segment
//! straight into their component plane, without a coefficient image. Blocks
//! equal to the plane's background are not stored. Samples use a certified
//! f32 transform; a sample whose certificate does not fix its byte is
//! recomputed in binary64 in the reference's order (T.81 A.3.3), so every
//! sample is the reference's byte.
mod basis;
mod plane;
mod spatial;

pub(super) use plane::Plane;

use super::{DctImage, Decoder, frame::Frame, frame::Storage};
use crate::deferred::Planar;
use pagura_core::{Diagnostics, Error, Limits, Result};

/// How a frame decodes into native planes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Native {
    /// Whether planes store rows and columns exchanged.
    pub(crate) transposed: bool,
}

/// The result of a deferred decode.
pub(crate) enum Deferred {
    /// A YCbCr frame in its component planes.
    Planar(Box<Planar>),
    /// A one-component frame, byte for byte as the ordinary decoder.
    Gray(DctImage),
}

/// Bytes of the per-component prefix table that joins a DC difference of
/// zero with the following end of block.
const SHORTCUT_BYTES: u64 = 4096 * 4;

/// Data units from which a frame's native AC lookups join coefficient
/// pairs; smaller frames do not repay building them.
const PAIRED_BLOCKS: usize = 4096;

/// Whether native AC lookups join coefficient pairs for `frame`, or for a
/// frame not yet defined.
pub(super) fn paired(frame: Option<&Frame>) -> bool {
    frame.is_none_or(|frame| {
        let blocks = frame.components.iter().map(|c| c.stride * c.rows);
        blocks.sum::<usize>() >= PAIRED_BLOCKS
    })
}

/// How a deferred decode ended.
pub(super) enum Finished {
    Done(Deferred),
    /// The frame does not qualify after all.
    Declined,
}

/// Decodes a baseline YCbCr or one-component JPEG into native planes, or
/// returns None when the frame does not qualify or decoding fails. The
/// caller then uses the ordinary decoder, which reports its own errors and
/// repairs; this decoder's diagnostics are kept only on success.
pub(crate) fn decode_deferred(
    data: &[u8],
    color_transform: Option<i64>,
    limits: &Limits,
    native: Native,
    diagnostics: &mut Diagnostics,
) -> Option<Deferred> {
    if data.len() > limits.stream_bytes {
        return None;
    }
    let mut scratch = Diagnostics::new(diagnostics.mode());
    let outcome = super::run(data, color_transform, limits, &mut scratch, Some(native))
        .and_then(|decoder| finish(decoder, color_transform, limits));
    let Ok(Finished::Done(result)) = outcome else {
        return None;
    };
    // The prerequisite core API validates the receiving recovery mode. If it
    // declines diagnostics, use the ordinary decoder to report the failure.
    if diagnostics.absorb(scratch.take()).is_err() {
        return None;
    }
    Some(result)
}

/// Whether a frame's colour interpretation and geometry suit native planes.
fn qualifies(frame: &Frame, adobe: Option<u8>, color_transform: Option<i64>) -> bool {
    if !frame.process.is_baseline() || frame.precision != 8 {
        return false;
    }
    match frame.components.as_slice() {
        [_] => true,
        [luma, cb, cr] => {
            matches!(adobe, None | Some(1))
                && (adobe.is_some() || color_transform.unwrap_or(1) == 1)
                && (luma.h, luma.v) == (frame.max_h, frame.max_v)
                && (cb.h, cb.v) == (cr.h, cr.v)
                && frame.max_h.is_multiple_of(cb.h)
                && matches!(frame.max_h / cb.h, 1 | 2 | 4)
        }
        _ => false,
    }
}

/// Replaces a qualifying frame's sample planes with native planes.
pub(super) fn prepare(
    frame: &mut Frame,
    (adobe, color_transform): (Option<u8>, Option<i64>),
    limits: &Limits,
    native: Native,
) -> Result<()> {
    if !qualifies(frame, adobe, color_transform) {
        return Err(Error::unsupported(
            crate::features::JPEG_PROCESS,
            "JPEG frame does not qualify for native planes",
        ));
    }
    // The frame's admission budgets one byte per sample of a sequential
    // plane; native planes also need a block flag and a prefix table.
    let tables = SHORTCUT_BYTES * frame.components.len() as u64;
    let planes: u64 = frame
        .components
        .iter()
        .map(|c| (c.stride * c.rows) as u64 * 65)
        .sum();
    if planes.saturating_add(tables) > limits.decoded_bytes as u64 {
        return Err(Error::limit("JPEG native plane budget exceeded"));
    }
    for (index, c) in frame.components.iter_mut().enumerate() {
        c.storage = Storage::Native(Box::new(Plane::new(
            plane::Shape {
                stride: c.stride,
                rows: c.rows,
                transposed: native.transposed,
            },
            if index == 0 { 255 } else { 128 },
        )?));
    }
    Ok(())
}

/// Completes a deferred frame at its end of image.
fn finish(mut decoder: Decoder, color_transform: Option<i64>, limits: &Limits) -> Result<Finished> {
    let mut frame = decoder
        .frame
        .take()
        .ok_or(Error::syntax("JPEG has no frame"))?;
    if !qualifies(&frame, decoder.adobe, color_transform)
        || frame
            .components
            .iter()
            .any(|c| !matches!(c.storage, Storage::Native(_)) || c.approximation[0] < 0)
    {
        return Ok(Finished::Declined);
    }
    if frame.components.len() == 1 {
        return gray(&frame, limits).map(|image| Finished::Done(Deferred::Gray(image)));
    }
    let planar = take_planar(&mut frame)?;
    Ok(Finished::Done(Deferred::Planar(Box::new(planar))))
}

fn gray(frame: &Frame, limits: &Limits) -> Result<DctImage> {
    let Storage::Native(plane) = &frame.components[0].storage else {
        return Err(Error::syntax("missing JPEG component plane"));
    };
    let (mut width, mut height) = (frame.width, frame.height);
    if plane.transposed() {
        (width, height) = (height, width);
    }
    let count = crate::parameters::budget(width, height, 1, 8, limits)?;
    let rows = height as usize;
    let width = width as usize;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("JPEG sample allocation failed"))?;
    for row in plane.samples().chunks_exact(plane.pitch()).take(rows) {
        samples.extend_from_slice(&row[..width]);
    }
    Ok(DctImage {
        width: u32::try_from(width).unwrap_or(u32::MAX),
        height: u32::try_from(rows).unwrap_or(u32::MAX),
        components: 1,
        samples,
    })
}

fn take_planar(frame: &mut Frame) -> Result<Planar> {
    let transposed = frame
        .components
        .iter()
        .any(|c| matches!(&c.storage, Storage::Native(plane) if plane.transposed()));
    let geometry = std::array::from_fn(|i| {
        let c = &frame.components[i];
        if transposed {
            [c.v, c.h, c.rows * 8]
        } else {
            [c.h, c.v, c.stride * 8]
        }
    });
    let mut planes = std::array::from_fn(|_| Vec::new());
    let mut dirty = std::array::from_fn(|_| Vec::new());
    let mut backgrounds = [0; 3];
    for (i, component) in frame.components.iter_mut().enumerate() {
        let Storage::Native(plane) = &mut component.storage else {
            return Err(Error::syntax("missing JPEG component plane"));
        };
        backgrounds[i] = plane.background();
        dirty[i] = plane.take_dirty();
        planes[i] = plane.take_samples();
    }
    let (width, height) = (frame.width as usize, frame.height as usize);
    let (width, height, maximum) = if transposed {
        (height, width, [frame.max_v, frame.max_h])
    } else {
        (width, height, [frame.max_h, frame.max_v])
    };
    Ok(Planar {
        width,
        height,
        maximum,
        geometry,
        planes,
        dirty,
        backgrounds,
        clean: Vec::new(),
        clean_words: 0,
        background_rgb: [0; 3],
    }
    .with_clean())
}

#[cfg(test)]
mod tests;
