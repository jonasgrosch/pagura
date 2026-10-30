//! Samples from the data of an image source: the codec part of decoding,
//! then the entries of finishing that the header already validated.

use super::header::{Codec, Header};
use super::{Layout, Orientation};
use crate::{
    DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, dispatch, fax_image, parameters as p,
};
use pagura_core::{Diagnostics, Error, Limits, Result};
use std::borrow::Cow;

/// Decodes `data` as `header` describes it, in `layout` and `orientation`.
pub(crate) fn decode(
    header: &Header,
    data: &[u8],
    (layout, orientation): (Layout, Orientation),
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<DecodedImage> {
    let transposed = orientation == Orientation::Transposed;
    let mut exchanged = false;
    let deferred = layout == Layout::Native;
    let (width, height) = (header.width, header.height);
    let (components, bits) = (header.declared_components, header.declared_bits);
    // Rows that damaged filter data left out are absent, not zero: decoding
    // does not report them as short data, and `absent_rows` hides them.
    let present = present_rows(header, data);
    let mut quiet = Diagnostics::new(pagura_core::Recovery::Repair);
    let diagnostics = if present.is_some() {
        &mut quiet
    } else {
        diagnostics
    };
    let mut image = DecodedImage {
        width,
        height,
        components,
        bits_per_component: 1,
        samples: Samples::Bytes(Vec::new()),
        kind: header.initial.clone(),
        decode: None,
        mask: MaskKind::None,
        interpolate: false,
    };
    match &header.codec {
        Codec::None if deferred && bits == 1 && components == 1 => {
            image.samples = p::packed(data, width, height, limits, diagnostics)?;
        }
        Codec::None => {
            image.bits_per_component = bits;
            image.samples = dispatch::unfiltered(
                Cow::Borrowed(data),
                (width, height, components, bits),
                limits,
                diagnostics,
            )?;
        }
        Codec::Dct(params) => {
            // A baseline YCbCr frame decodes faster into its native planes;
            // their rows convert to exactly the interleaved samples.
            let planes = layout == Layout::Interleaved && bits == 8;
            let options = dispatch::DctOptions {
                deferred: deferred || planes,
                // Interleaved samples come from planes in image rows.
                transposed: transposed && !planes,
            };
            exchanged = dispatch::decode_dct(
                data,
                params.as_ref(),
                (&mut image, bits),
                options,
                limits,
                diagnostics,
            )?;
            if planes {
                interleave(&mut image.samples, limits)?;
            }
        }
        Codec::Ccitt(params) => {
            image.samples = fax_image::decode(
                data,
                params.as_ref(),
                (width, height, deferred),
                limits,
                diagnostics,
            )?;
        }
        Codec::Jbig2(globals) => {
            image.samples =
                dispatch::jbig2_samples(data, globals.as_deref(), (width, height), limits)?;
        }
        Codec::Jpx(jpx) => jpx.decode(data, &mut image, limits, diagnostics)?,
    }
    let mut image = finish(header, image)?;
    if let Some(rows) = present {
        image = absent_rows(header, image, rows, limits)?;
    }
    if transposed && !exchanged {
        return image.transposed(limits);
    }
    Ok(image)
}

/// The whole rows of unfiltered `data` when damaged filter data cut it short
/// of the declared samples (`filter.corrupt`); `None` when no row is missing.
fn present_rows(header: &Header, data: &[u8]) -> Option<usize> {
    if !header.damaged || !matches!(header.codec, Codec::None) {
        return None;
    }
    let row_bits = u64::from(header.width)
        * u64::from(header.declared_components)
        * u64::from(header.declared_bits);
    let row_bytes = usize::try_from(row_bits.div_ceil(8)).ok()?;
    let rows = data.len().checked_div(row_bytes)?;
    (rows < usize::try_from(header.height).ok()?).then_some(rows)
}

/// Leaves the rows from `rows` on absent rather than zero-filled: a stencil
/// does not paint them, and a colour image gets an opacity plane that makes
/// them transparent. An image with a mask of its own keeps an error, as
/// combining the two would invent a mask.
fn absent_rows(
    header: &Header,
    mut image: DecodedImage,
    rows: usize,
    limits: &Limits,
) -> Result<DecodedImage> {
    let width = usize::try_from(image.width).map_err(|_| Error::limit("image row overflow"))?;
    if header.stencil() {
        // The sample that does not paint is 1, or 0 under `/Decode [1 0]`,
        // which zero-filling already left.
        if !matches!(image.kind, ImageKind::Stencil { inverted: true }) {
            match &mut image.samples {
                Samples::Packed(packed) => {
                    let start = rows.saturating_mul(packed.stride).min(packed.bytes.len());
                    packed.bytes[start..].fill(0xFF);
                }
                Samples::Bytes(values) => {
                    let start = rows.saturating_mul(width).min(values.len());
                    values[start..].fill(1);
                }
                _ => return Err(Error::syntax("damaged stencil samples")),
            }
        }
        return Ok(image);
    }
    if image.mask != MaskKind::None {
        return Err(Error::syntax(
            "damaged image data under a mask of its own cannot be shown",
        ));
    }
    let count = p::budget(image.width, image.height, 1, 1, limits)?;
    let mut alpha = Vec::new();
    alpha
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("image opacity allocation failed"))?;
    alpha.resize(rows.saturating_mul(width).min(count), 1);
    alpha.resize(count, 0);
    image.mask = MaskKind::Embedded(ImageAlpha {
        bits_per_component: 1,
        samples: Samples::Bytes(alpha),
        premultiplied: false,
    });
    Ok(image)
}

/// Converts a JPEG's native planes into interleaved RGB samples, row by
/// row with Table 13 as the interleaved decoder converts them.
fn interleave(samples: &mut Samples, limits: &Limits) -> Result<()> {
    let Samples::Planar(planar) = samples else {
        return Ok(());
    };
    let row = planar
        .width()
        .checked_mul(3)
        .ok_or(Error::limit("image row overflow"))?;
    let length = row
        .checked_mul(planar.height())
        .filter(|&length| length <= limits.decoded_bytes)
        .ok_or(Error::limit("image sample budget exceeded"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::limit("image sample allocation failed"))?;
    bytes.resize(length, 0);
    for (y, out) in bytes.chunks_exact_mut(row).enumerate() {
        if !planar.rgb(y, out) {
            return Err(Error::syntax("JPEG planes do not cover the image"));
        }
    }
    *samples = Samples::Bytes(bytes);
    Ok(())
}

/// The entries `finish` adds to a decoded image, already validated by the
/// header, and the stencil precision only decoding reveals.
fn finish(header: &Header, mut image: DecodedImage) -> Result<DecodedImage> {
    image.decode.clone_from(&header.decode);
    image.interpolate = header.interpolate;
    if header.stencil() {
        if image.bits_per_component != 1 {
            return Err(Error::syntax("invalid stencil image dictionary"));
        }
        image.kind = header.kind.clone();
        return Ok(image);
    }
    if !matches!(image.mask, MaskKind::Embedded(_)) {
        image.mask = header.mask_kind();
    }
    Ok(image)
}
