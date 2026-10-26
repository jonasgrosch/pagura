//! How an image's samples are painted (8.9.5, 8.9.6): the colour space
//! resolved from the source's `ColorSpace` value or JPEG 2000 colour, the
//! decode ranges, the intent and the masks, described without decoding.

use super::super::Machine;
use crate::device::Device;
use crate::features;
use crate::model::{Image, ImageMask};
use pagura_color::{ColorSpace, Components, Intent, MAX_COMPONENTS};
use pagura_core::{Dictionary, Error, ObjectId, Result};
use pagura_document::source::ReadResult;
use pagura_image::jpx::JpxColor;
use pagura_image::{ImageId, ImageKind, ImageSource, SourceMask};
use std::sync::Arc;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// Describes the image `source`, whose colour space names refer to
    /// `resources`; `stack` holds the images whose masks are being read.
    pub(in crate::machine) fn describe(
        &mut self,
        source: ImageSource,
        resources: &Dictionary,
        stack: &mut Vec<ObjectId>,
    ) -> ReadResult<Arc<Image>> {
        let components = usize::from(source.components());
        if components == 0 || components > MAX_COMPONENTS {
            return Err(Error::limit("image component count exceeds painter capacity").into());
        }
        let space = self.image_space(&source, resources)?;
        if let Some(space) = &space
            && (matches!(space.as_ref(), ColorSpace::Pattern(_))
                || space.components() != components)
        {
            return Err(Error::syntax("image colour space component mismatch").into());
        }
        let mask = match source.mask() {
            SourceMask::Soft { object, matte } => {
                let mask = self.xobject_image(*object, resources, stack)?;
                let matte = matte
                    .as_deref()
                    .map(|matte| check_matte(matte, space.as_deref(), &source, &mask))
                    .transpose()?;
                ImageMask::Soft { mask, matte }
            }
            SourceMask::Stencil(object) => {
                ImageMask::Stencil(self.xobject_image(*object, resources, stack)?)
            }
            SourceMask::ColorKey(keys) => ImageMask::ColorKey(keys.clone().into()),
            SourceMask::Embedded { .. } => ImageMask::Embedded,
            _ => ImageMask::None,
        };
        let decode = decode(&source, space.as_deref())?;
        let intent = if space.is_some() {
            source
                .intent()
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_name()
                        .map(|name| Intent::from_name(name.as_bytes()))
                        .ok_or(Error::syntax("image Intent must be a name"))
                })
                .transpose()?
        } else {
            None
        };
        Ok(Arc::new(Image {
            interpolate: source.interpolate(),
            source: Arc::new(source),
            space,
            decode: decode.into(),
            intent,
            mask,
        }))
    }

    /// The image's colour space: its `ColorSpace` value, the device space
    /// of a JPEG 2000 container, an embedded ICC profile, or none for a
    /// stencil mask. Explicit PDF colour always wins (7.4.9, Table 87).
    fn image_space(
        &mut self,
        source: &ImageSource,
        resources: &Dictionary,
    ) -> ReadResult<Option<Arc<ColorSpace>>> {
        match source.kind() {
            ImageKind::Color {
                color_space: Some(value),
            } => self.session.colors.resolve(resources, value).map(Some),
            ImageKind::EmbeddedColor {
                color: JpxColor::Icc(data),
            } => {
                let ImageId::Object(id) = source.id() else {
                    return Err(Error::syntax("JPXDecode is forbidden in inline images").into());
                };
                self.session
                    .colors
                    .embedded_icc(id, data, source.components())
                    .map(Some)
            }
            ImageKind::EmbeddedColor {
                color: JpxColor::Enumerated(18),
            } if (0..3).all(|index| source.component_bits(index) == Some(8)) => {
                Ok(Some(Arc::new(ColorSpace::Sycc)))
            }
            ImageKind::EmbeddedColor { color } => {
                let name: &[u8] = match (color, source.components()) {
                    (JpxColor::Enumerated(16), 3) => b"DeviceRGB",
                    (JpxColor::Enumerated(17), 1) => b"DeviceGray",
                    _ => {
                        return Err(Error::unsupported(
                            features::XOBJECT_JPX_COLOR,
                            "embedded JPX colour interpretation is not implemented",
                        )
                        .into());
                    }
                };
                self.session.colors.named(resources, name).map(Some)
            }
            ImageKind::Stencil { .. } => Ok(None),
            ImageKind::Color { color_space: None } => {
                Err(Error::syntax("image has no colour space").into())
            }
        }
    }
}

/// `Decode`, or the default ranges of the space (8.9.5.2, Table 88).
fn decode(source: &ImageSource, space: Option<&ColorSpace>) -> Result<Vec<[f64; 2]>> {
    let components = usize::from(source.components());
    let mut ranges = Vec::with_capacity(components);
    for component in 0..components {
        let range = if let Some(values) = source.decode_array() {
            let value = |index: usize| {
                values
                    .get(index)
                    .copied()
                    .ok_or(Error::syntax("short image Decode"))
            };
            [value(2 * component)?, value(2 * component + 1)?]
        } else {
            match space {
                Some(ColorSpace::Indexed(_)) => [0.0, f64::from(max_sample(source, component)?)],
                Some(ColorSpace::Calibrated(cie)) => {
                    cie.lab_range().map_or([0.0, 1.0], |r| match component {
                        0 => [0.0, 100.0],
                        1 => [r[0], r[1]],
                        _ => [r[2], r[3]],
                    })
                }
                Some(ColorSpace::Icc(icc)) => {
                    icc.range.get(component).copied().unwrap_or([0.0, 1.0])
                }
                _ => [0.0, 1.0],
            }
        };
        ranges.push(range);
    }
    Ok(ranges)
}

/// The largest raw sample of `component`.
fn max_sample(source: &ImageSource, component: usize) -> Result<u16> {
    let bits = source
        .component_bits(component)
        .filter(|b| (1..=16).contains(b))
        .ok_or(Error::syntax("invalid image component precision"))?;
    Ok(u16::MAX >> (16 - u32::from(bits)))
}

/// `Matte` (11.6.5.3): one value per component of the base space, for a
/// soft mask with the image's dimensions.
fn check_matte(
    matte: &[f64],
    space: Option<&ColorSpace>,
    source: &ImageSource,
    mask: &Image,
) -> Result<Components> {
    let count = space.map_or(0, |s| match s {
        ColorSpace::Indexed(indexed) => indexed.base.components(),
        s => s.components(),
    });
    if matte.len() != count {
        return Err(Error::syntax("invalid image Matte"));
    }
    if mask.source.width() != source.width() || mask.source.height() != source.height() {
        return Err(Error::syntax("Matte mask dimensions differ from image"));
    }
    Ok(Components::from_slice(matte))
}
