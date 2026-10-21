//! Samples from the data of an image source: the codec part of decoding,
//! then the entries of finishing that the header already validated.

use super::header::{Codec, Header};
use crate::deferred::SampleLayout;
use crate::{DecodedImage, MaskKind, Samples, dispatch, fax_image, parameters as p};
use pagura_core::{Diagnostics, Error, Limits, Result};
use std::borrow::Cow;

/// Decodes `data` as `header` describes it, in `layout`.
pub(crate) fn decode(
    header: &Header,
    data: &[u8],
    layout: SampleLayout,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<DecodedImage> {
    let (deferred, cells) = (layout == SampleLayout::Deferred, layout.cells());
    let (width, height) = (header.width, header.height);
    let (components, bits) = (header.declared_components, header.declared_bits);
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
        reduction: None,
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
            dispatch::decode_dct(
                data,
                params.as_ref(),
                (&mut image, bits),
                (deferred, cells, None),
                limits,
                diagnostics,
            )?;
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
    finish(header, image)
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
