//! The decoded image model: samples plus everything needed to paint them.
use crate::JpxColor;
use crate::adapter::Pixels;
use pagura_core::{Error, ObjectId, Result, Value};
use std::ops::RangeInclusive;

/// How an image's samples are interpreted.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ImageKind {
    /// Color samples in a resolved PDF color space.
    Color {
        /// The `/ColorSpace` value, resolved, or the device space a JPEG
        /// 2000 container's enumerated space stands for (7.4.9).
        color_space: Value,
    },
    /// Color interpretation supplied by the JPEG 2000 container rather than
    /// a PDF color-space value (7.4.9). Profiles and enumerated spaces are
    /// retained so painting can apply their original interpretation.
    EmbeddedColor {
        /// Color description from the container.
        color: JpxColor,
    },
    /// A 1-bit stencil mask painted with the current fill color
    /// (`/ImageMask true`); `inverted` follows `/Decode [1 0]`.
    Stencil {
        /// Whether sample 1 (rather than 0) paints.
        inverted: bool,
    },
}

/// The masking of a color image (8.9.6).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MaskKind {
    /// No mask.
    None,
    /// Color-key masking: an inclusive sample range per component.
    ColorKey(Vec<RangeInclusive<u16>>),
    /// An explicit stencil-mask image XObject.
    Stencil(ObjectId),
    /// A soft-mask image XObject (`/SMask`).
    Soft(ObjectId),
    /// An opacity channel decoded from the image data itself (JPEG 2000
    /// with `/SMaskInData`, 7.4.9), separate from the color samples.
    Embedded(ImageAlpha),
}

/// A JPEG 2000 opacity plane (7.4.9 and Table 87). It has one sample per
/// image pixel. Samples are raw integers; normalize by their precision at
/// painting time. The alpha-is-shape graphics state remains a paint concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAlpha {
    bits_per_component: u8,
    samples: Samples,
    premultiplied: bool,
}

/// Whether the color samples have been multiplied by the opacity (7.4.9,
/// `SMaskInData` 1 and 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Association {
    Straight,
    Premultiplied,
}

impl ImageAlpha {
    pub(crate) fn new(bits_per_component: u8, samples: Samples, association: Association) -> Self {
        Self {
            bits_per_component,
            samples,
            premultiplied: association == Association::Premultiplied,
        }
    }

    /// Opacity precision, independent of the color components' precision.
    pub const fn bits_per_component(&self) -> u8 {
        self.bits_per_component
    }

    /// One opacity sample per image pixel, in source row order.
    pub const fn samples(&self) -> &Samples {
        &self.samples
    }

    /// Whether the color samples are already multiplied by this opacity
    /// (`SMaskInData` 2); unpremultiplication is deferred to painting.
    pub const fn premultiplied(&self) -> bool {
        self.premultiplied
    }
}

/// Unpacked samples, one value per component. Values of up to 8 bits use
/// one byte each, values of 9 to 16 bits (and every value of an image whose
/// components differ in precision) two bytes. The set is closed: PDF
/// samples have at most 16 bits (8.9.5.1, 7.4.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    /// Values of 1 to 8 bits.
    Bytes(Vec<u8>),
    /// Values of 9 to 16 bits, or of mixed precision.
    Words(Vec<u16>),
}

impl Samples {
    /// Number of values.
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) => values.len(),
        }
    }

    /// Whether there are no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value at `index`, widened to `u16`.
    pub fn get(&self, index: usize) -> Option<u16> {
        match self {
            Self::Bytes(values) => values.get(index).copied().map(u16::from),
            Self::Words(values) => values.get(index).copied(),
        }
    }
}

/// The bits per component of an image's color samples.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Precision {
    /// Every component has this many bits: 1, 2, 4, 8 or 16 for ordinary
    /// PDF samples, 1 to 16 for JPEG 2000.
    Uniform(u8),
    /// JPEG 2000 components of different precisions (7.4.9, 8.9.5.2), one
    /// entry per component in interleave order. The samples are words and
    /// no precision conversion is applied.
    Mixed(Vec<u8>),
}

/// A decoded image: unpacked samples plus painting parameters.
///
/// The fields agree with each other: there are `width × height ×
/// components` samples in the representation the precision calls for, a
/// decode array has two entries per component, a color key one range per
/// component, an embedded opacity plane one sample per pixel, and a stencil
/// has one 1-bit component.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    width: u32,
    height: u32,
    components: u8,
    precision: Precision,
    samples: Samples,
    kind: ImageKind,
    decode: Option<Vec<f64>>,
    mask: MaskKind,
    interpolate: bool,
}

/// A decoded image's parts, checked against the dictionary by the caller.
pub(crate) struct Parts {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Pixels,
    pub(crate) kind: ImageKind,
    pub(crate) decode: Option<Vec<f64>>,
    pub(crate) mask: MaskKind,
    pub(crate) interpolate: bool,
}

impl DecodedImage {
    /// Assembles an image whose parts the dispatcher has validated.
    pub(crate) fn assemble(parts: Parts) -> Self {
        let Parts {
            width,
            height,
            pixels,
            kind,
            decode,
            mask,
            interpolate,
        } = parts;
        let image = Self {
            width,
            height,
            components: pixels.components,
            precision: pixels.precision,
            samples: pixels.samples,
            kind,
            decode,
            mask,
            interpolate,
        };
        debug_assert!(image.consistent(), "inconsistent decoded image");
        image
    }

    /// Whether the fields agree as the type's documentation states.
    fn consistent(&self) -> bool {
        let checked = Self::new(
            self.width,
            self.height,
            self.components,
            self.precision.clone(),
            self.samples.clone(),
            self.kind.clone(),
        )
        .and_then(|image| match &self.decode {
            Some(decode) => image.with_decode(decode.clone()),
            None => Ok(image),
        })
        .and_then(|image| image.with_mask(self.mask.clone()));
        checked.is_ok()
    }

    /// An unmasked image without a decode array or interpolation.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error when the samples, precision, component
    /// count and kind disagree (see the type's documentation).
    pub fn new(
        width: u32,
        height: u32,
        components: u8,
        precision: Precision,
        samples: Samples,
        kind: ImageKind,
    ) -> Result<Self> {
        let pixels = u64::from(width) * u64::from(height);
        if usize::try_from(pixels * u64::from(components)).ok() != Some(samples.len()) {
            return Err(Error::invalid_argument(
                "image sample count disagrees with its size",
            ));
        }
        let representable = match (&precision, &samples) {
            (Precision::Uniform(1..=8), Samples::Bytes(_))
            | (Precision::Uniform(9..=16), Samples::Words(_)) => true,
            (Precision::Mixed(bits), Samples::Words(_)) => {
                bits.len() == usize::from(components) && bits.iter().all(|b| (1..=16).contains(b))
            }
            _ => false,
        };
        if !representable || components == 0 {
            return Err(Error::invalid_argument(
                "image precision disagrees with its samples",
            ));
        }
        if matches!(kind, ImageKind::Stencil { .. })
            && (components != 1 || precision != Precision::Uniform(1))
        {
            return Err(Error::invalid_argument(
                "stencil images have one 1-bit component",
            ));
        }
        Ok(Self {
            width,
            height,
            components,
            precision,
            samples,
            kind,
            decode: None,
            mask: MaskKind::None,
            interpolate: false,
        })
    }

    /// The image with a `/Decode` array.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error unless there are two finite values per
    /// component.
    pub fn with_decode(mut self, decode: Vec<f64>) -> Result<Self> {
        if decode.len() != usize::from(self.components) * 2 || !decode.iter().all(|v| v.is_finite())
        {
            return Err(Error::invalid_argument("invalid image decode array"));
        }
        self.decode = Some(decode);
        Ok(self)
    }

    /// The image with masking.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error for a mask on a stencil, a color key
    /// without one range per component, or an opacity plane without one
    /// sample per pixel.
    pub fn with_mask(mut self, mask: MaskKind) -> Result<Self> {
        let fits = match &mask {
            MaskKind::None => true,
            _ if matches!(self.kind, ImageKind::Stencil { .. }) => false,
            MaskKind::ColorKey(ranges) => ranges.len() == usize::from(self.components),
            MaskKind::Embedded(alpha) => {
                usize::try_from(u64::from(self.width) * u64::from(self.height)).ok()
                    == Some(alpha.samples.len())
            }
            MaskKind::Stencil(_) | MaskKind::Soft(_) => true,
        };
        if !fits {
            return Err(Error::invalid_argument(
                "image mask disagrees with the image",
            ));
        }
        self.mask = mask;
        Ok(self)
    }

    /// The image with `/Interpolate true`.
    #[must_use]
    pub const fn interpolated(mut self) -> Self {
        self.interpolate = true;
        self
    }

    /// Width in samples.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in samples.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Color components per pixel.
    pub const fn components(&self) -> u8 {
        self.components
    }

    /// The precision of the color components.
    pub const fn precision(&self) -> &Precision {
        &self.precision
    }

    /// Bits per component when every component has the same precision.
    pub const fn bits_per_component(&self) -> Option<u8> {
        match self.precision {
            Precision::Uniform(bits) => Some(bits),
            Precision::Mixed(_) => None,
        }
    }

    /// Precision of the color component at `index`, or None when that
    /// component does not exist. Opacity has its own precision.
    pub fn component_bits(&self, index: usize) -> Option<u8> {
        if index >= usize::from(self.components) {
            return None;
        }
        match &self.precision {
            Precision::Uniform(bits) => Some(*bits),
            Precision::Mixed(bits) => bits.get(index).copied(),
        }
    }

    /// Row-major samples, top row first, components interleaved.
    pub const fn samples(&self) -> &Samples {
        &self.samples
    }

    /// How the samples are interpreted.
    pub const fn kind(&self) -> &ImageKind {
        &self.kind
    }

    /// The `/Decode` array, if present.
    pub fn decode(&self) -> Option<&[f64]> {
        self.decode.as_deref()
    }

    /// Masking.
    pub const fn mask(&self) -> &MaskKind {
        &self.mask
    }

    /// `/Interpolate`.
    pub const fn interpolate(&self) -> bool {
        self.interpolate
    }

    /// Device fallback for an embedded color description that the painter
    /// cannot support (7.4.9). The original profile or enumerated space
    /// remains available in [`ImageKind::EmbeddedColor`].
    pub fn embedded_color_fallback(&self) -> Option<Value> {
        if !matches!(self.kind, ImageKind::EmbeddedColor { .. }) {
            return None;
        }
        match self.components {
            1 => Some(Value::name(b"DeviceGray")),
            3 => Some(Value::name(b"DeviceRGB")),
            4 => Some(Value::name(b"DeviceCMYK")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Association, DecodedImage, ImageAlpha, ImageKind, MaskKind, Precision, Samples};
    use pagura_core::{ErrorKind, ObjectId, Value};

    fn gray(samples: Samples, precision: Precision) -> pagura_core::Result<DecodedImage> {
        DecodedImage::new(
            2,
            1,
            1,
            precision,
            samples,
            ImageKind::Color {
                color_space: Value::name(b"DeviceGray"),
            },
        )
    }

    #[test]
    fn construction_checks_samples_against_size_and_precision() {
        let image = gray(Samples::Bytes(vec![0, 255]), Precision::Uniform(8)).unwrap();
        assert_eq!(image.bits_per_component(), Some(8));
        assert_eq!(image.component_bits(0), Some(8));
        assert_eq!(image.component_bits(1), None);
        for (samples, precision) in [
            (Samples::Bytes(vec![0]), Precision::Uniform(8)),
            (Samples::Words(vec![0, 1]), Precision::Uniform(8)),
            (Samples::Bytes(vec![0, 1]), Precision::Uniform(16)),
            (Samples::Words(vec![0, 1]), Precision::Mixed(vec![3, 9])),
            (Samples::Bytes(vec![0, 1]), Precision::Uniform(0)),
        ] {
            assert_eq!(
                gray(samples, precision).unwrap_err().kind,
                ErrorKind::InvalidArgument
            );
        }
        let mixed = gray(Samples::Words(vec![0, 1]), Precision::Mixed(vec![12])).unwrap();
        assert_eq!(mixed.bits_per_component(), None);
        assert_eq!(mixed.component_bits(0), Some(12));
        let stencil = DecodedImage::new(
            2,
            1,
            1,
            Precision::Uniform(8),
            Samples::Bytes(vec![0, 1]),
            ImageKind::Stencil { inverted: false },
        );
        assert_eq!(stencil.unwrap_err().kind, ErrorKind::InvalidArgument);
    }

    #[test]
    fn decode_arrays_and_masks_match_the_components() {
        let image = gray(Samples::Bytes(vec![0, 255]), Precision::Uniform(8)).unwrap();
        assert!(image.clone().with_decode(vec![1.0, 0.0]).is_ok());
        assert!(image.clone().with_decode(vec![1.0]).is_err());
        assert!(image.clone().with_decode(vec![f64::NAN, 0.0]).is_err());
        assert!(
            image
                .clone()
                .with_mask(MaskKind::ColorKey(vec![std::ops::RangeInclusive::new(
                    0, 3
                )]))
                .is_ok()
        );
        assert!(
            image
                .clone()
                .with_mask(MaskKind::ColorKey(Vec::new()))
                .is_err()
        );
        let alpha =
            |count| ImageAlpha::new(8, Samples::Bytes(vec![0; count]), Association::Straight);
        assert!(
            image
                .clone()
                .with_mask(MaskKind::Embedded(alpha(2)))
                .is_ok()
        );
        assert!(
            image
                .clone()
                .with_mask(MaskKind::Embedded(alpha(1)))
                .is_err()
        );
        let stencil = DecodedImage::new(
            2,
            1,
            1,
            Precision::Uniform(1),
            Samples::Bytes(vec![0, 1]),
            ImageKind::Stencil { inverted: true },
        )
        .unwrap();
        let mask = MaskKind::Soft(ObjectId::new(4, 0));
        assert!(stencil.with_mask(mask).is_err());
        assert!(image.interpolated().interpolate());
    }
}
