//! A one-bit image minified by exactly two on a cell-aligned grid: each
//! device pixel is the exact mean of two-by-two source samples.
use super::{Placement, footprint::Footprint};
use crate::{Pixmap, RenderOptions, interpreter::images::Image};
use pagura_core::{Error, Limits, Result};
use pagura_image::{DecodedImage, Samples};
fn axis(range: std::ops::Range<usize>, scale: f64, offset: f64, maximum: usize) -> bool {
    if (scale.abs() - 2.0).abs() > 1e-10 {
        return false;
    }
    for pixel in range {
        let span = Footprint::new(pixel, scale, offset, maximum, false);
        let mut first = span.first;
        let mut end = span.end;
        while first < end && span.weight(first) < 1e-10 {
            first += 1;
        }
        while end > first && span.weight(end - 1) < 1e-10 {
            end -= 1;
        }
        if end - first != 2
            || !first.is_multiple_of(2)
            || (span.weight(first) - 0.5).abs() > 1e-10
            || (span.weight(end - 1) - 0.5).abs() > 1e-10
        {
            return false;
        }
    }
    true
}
pub(super) fn paint(
    image: &Image,
    p: &Placement,
    options: &RenderOptions,
    limits: Limits,
) -> Result<Option<Pixmap>> {
    let Samples::Packed(source) = &image.decoded.samples else {
        return Ok(None);
    };
    if !source.width().is_multiple_of(2)
        || !source.height().is_multiple_of(2)
        || image
            .decode
            .iter()
            .any(|&(lo, hi)| !(0.0..=1.0).contains(&lo) || !(0.0..=1.0).contains(&hi))
    {
        return Ok(None);
    }
    let t = super::source_transform(image, p)?;
    let [left, right, top, bottom] = p.range;
    let valid = if p.rotated {
        axis(top..bottom, t.c, t.e, source.width()) && axis(left..right, t.b, t.f, source.height())
    } else {
        axis(left..right, t.a, t.e, source.width()) && axis(top..bottom, t.d, t.f, source.height())
    };
    if !valid {
        return Ok(None);
    }
    let width = source.width() / 2;
    let height = source.height() / 2;
    let count = width
        .checked_mul(height)
        .ok_or(Error::limit("aligned area size overflow"))?;
    let bytes = count
        .checked_add(image.decoded.samples.len().saturating_mul(2))
        .and_then(|n| {
            (p.width as usize)
                .checked_mul(p.height as usize)
                .and_then(|n| n.checked_mul(4))
                .and_then(|b| n.checked_add(b))
        })
        .and_then(|n| {
            (right - left)
                .checked_mul(size_of::<usize>() + 4)
                .and_then(|b| n.checked_add(b))
        })
        .and_then(|n| n.checked_add(4096))
        .filter(|&n| n <= limits.decoded_bytes);
    if bytes.is_none() {
        return Ok(None);
    }
    if image
        .decoded
        .samples
        .len()
        .checked_add(
            (right - left)
                .checked_mul(bottom - top)
                .ok_or(Error::limit("aligned area work overflow"))?,
        )
        .is_none_or(|n| n > limits.raster_work)
    {
        return Err(Error::limit("aligned area work exceeded"));
    }
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("aligned area allocation failed"))?;
    samples.resize(count, 0u8);
    for (y, row) in samples.chunks_exact_mut(width).enumerate() {
        let a = source
            .row(y * 2)
            .ok_or(Error::syntax("aligned row outside source"))?;
        let b = source
            .row(y * 2 + 1)
            .ok_or(Error::syntax("aligned row outside source"))?;
        if !super::binary::half(a, b, row) {
            return Err(Error::syntax("aligned binary row mismatch"));
        }
    }
    let reduced = Image {
        decoded: DecodedImage {
            width: u32::try_from(width).map_err(|_| Error::limit("aligned width overflow"))?,
            height: u32::try_from(height).map_err(|_| Error::limit("aligned height overflow"))?,
            bits_per_component: 8,
            components: 1,
            samples: Samples::Bytes(samples),
            kind: image.decoded.kind.clone(),
            decode: None,
            mask: pagura_image::MaskKind::None,
            interpolate: image.decoded.interpolate,
            reduction: None,
        },
        space: image.space.clone(),
        mask: None,
        matte: None,
        decode: image.decode.clone(),
        intent: image.intent,
    };
    let Samples::Bytes(samples) = &reduced.decoded.samples else {
        return Err(Error::syntax("aligned sample representation mismatch"));
    };
    super::nearest(&reduced, samples, p, options, limits).map(Some)
}
