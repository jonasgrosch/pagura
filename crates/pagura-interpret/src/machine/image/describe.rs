//! How an image's samples are painted (8.9.5, 8.9.6): colour space, decode
//! array, intent and masks, read from the image dictionary (abbreviated
//! keys and names of inline images included).

use super::super::Machine;
use crate::device::Device;
use crate::features;
use crate::model::{Image, ImageMask};
use pagura_color::{ColorSpace, Components, Intent, MAX_COMPONENTS};
use pagura_core::{Dictionary, Error, Name, ObjectId, Result, Value};
use pagura_document::Document;
use pagura_image::ImageSource;
use std::sync::Arc;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// Describes the image `source` whose dictionary is `dictionary`.
    pub(in crate::machine) fn describe(
        &mut self,
        source: ImageSource,
        dictionary: &Dictionary,
        resources: &Dictionary,
        stack: &mut Vec<ObjectId>,
    ) -> Result<Arc<Image>> {
        let document = self.document();
        let get = |long: &[u8], short: &[u8]| -> Result<Option<&Value>> {
            dictionary
                .get(long)
                .or_else(|| dictionary.get(short))
                .map(|v| document.resolve(v))
                .transpose()
                .map(|v| v.filter(|v| !v.is_null()))
        };
        let stencil = get(b"ImageMask", b"IM")?.and_then(Value::as_bool) == Some(true);
        let jpx = terminal_filter(document, dictionary)?.is_some_and(|n| n == b"JPXDecode");
        let space = if stencil {
            None
        } else {
            let value = get(b"ColorSpace", b"CS")?;
            let value = match value {
                Some(value) => expand(value),
                None if jpx => {
                    return Err(Error::unsupported(
                        features::XOBJECT_JPX_COLOR,
                        "embedded JPX colour interpretation is not implemented",
                    ));
                }
                None => return Err(Error::syntax("image has no colour space")),
            };
            let space = self.session.colors.resolve(resources, &value)?;
            if matches!(space.as_ref(), ColorSpace::Pattern(_)) {
                return Err(Error::syntax("image colour space component mismatch"));
            }
            Some(space)
        };
        let components = space.as_ref().map_or(1, |s| s.components());
        if components == 0 || components > MAX_COMPONENTS {
            return Err(Error::limit(
                "image component count exceeds painter capacity",
            ));
        }
        let bits = if stencil {
            1
        } else {
            get(b"BitsPerComponent", b"BPC")?
                .and_then(Value::as_integer)
                .and_then(|n| u8::try_from(n).ok())
                .filter(|n| matches!(n, 1 | 2 | 4 | 8 | 16))
                .unwrap_or(8)
        };
        let mask = self.mask(&source, dictionary, resources, space.as_ref(), (jpx, bits), stack)?;
        let decode = decode(document, get(b"Decode", b"D")?, space.as_deref(), components, bits)?;
        let intent = if space.is_some() {
            get(b"Intent", b"Intent")?
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
        let interpolate = get(b"Interpolate", b"I")?.and_then(Value::as_bool) == Some(true);
        Ok(Arc::new(Image {
            source: Arc::new(source),
            space,
            decode: decode.into(),
            interpolate,
            intent,
            mask,
        }))
    }

    /// The image's masking (8.9.6): a soft mask with its matte, an explicit
    /// stencil mask, a colour key, or alpha in JPEG 2000 data.
    fn mask(
        &mut self,
        source: &ImageSource,
        dictionary: &Dictionary,
        resources: &Dictionary,
        space: Option<&Arc<ColorSpace>>,
        (jpx, bits): (bool, u8),
        stack: &mut Vec<ObjectId>,
    ) -> Result<ImageMask> {
        let document = self.document();
        let entry = |key: &[u8]| -> Result<Option<&Value>> {
            Ok(dictionary.get(key).filter(|v| !document.resolve(v).is_ok_and(Value::is_null)))
        };
        if let Some(value) = entry(b"SMask")? {
            let id = value
                .as_reference()
                .ok_or(Error::syntax("SMask must reference an image stream"))?;
            let mask = self.xobject_image(id, resources, stack)?;
            let matte = document
                .object(id)?
                .dictionary()
                .and_then(|d| d.get(b"Matte"))
                .map(|value| matte(document, value, space, source, &mask))
                .transpose()?;
            return Ok(ImageMask::Soft { mask, matte });
        }
        if let Some(value) = entry(b"Mask")? {
            if let Some(id) = value.as_reference()
                && document.object(id)?.is_stream()
            {
                return Ok(ImageMask::Stencil(self.xobject_image(id, resources, stack)?));
            }
            let components = space.map_or(1, |s| s.components());
            return color_key(document, value, components, bits);
        }
        let in_data = dictionary
            .get(b"SMaskInData")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .unwrap_or(0);
        if jpx && in_data != 0 {
            return Ok(ImageMask::Embedded);
        }
        Ok(ImageMask::None)
    }
}

/// The last filter of the image's filter chain.
fn terminal_filter<'a>(document: &'a Document, dictionary: &'a Dictionary) -> Result<Option<&'a Name>> {
    let filter = dictionary
        .get(b"Filter")
        .or_else(|| dictionary.get(b"F"))
        .map(|v| document.resolve(v))
        .transpose()?;
    Ok(match filter {
        Some(Value::Array(filters)) => filters
            .last()
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name),
        Some(value) => value.as_name(),
        None => None,
    })
}

/// Expands the colour-space abbreviations of inline images (Table 92).
fn expand(value: &Value) -> Value {
    let name = |n: &Name| -> Option<&'static [u8]> {
        Some(match n.as_bytes() {
            b"G" => b"DeviceGray",
            b"RGB" => b"DeviceRGB",
            b"CMYK" => b"DeviceCMYK",
            b"I" => b"Indexed",
            _ => return None,
        })
    };
    match value {
        Value::Name(n) => name(n).map_or_else(|| value.clone(), Value::name),
        Value::Array(values) => {
            let mut values = values.clone();
            if let Some(first) = values.first_mut()
                && let Some(expanded) = first.as_name().and_then(name)
            {
                *first = Value::name(expanded);
            }
            if let Some(base) = values.get_mut(1) {
                *base = expand(base);
            }
            Value::Array(values)
        }
        value => value.clone(),
    }
}

/// `Decode`, or the default ranges of the space (8.9.5.2, Table 88).
fn decode(
    document: &Document,
    value: Option<&Value>,
    space: Option<&ColorSpace>,
    components: usize,
    bits: u8,
) -> Result<Vec<[f64; 2]>> {
    if let Some(value) = value {
        let values = document
            .resolve(value)?
            .as_array()
            .ok_or(Error::syntax("invalid image Decode"))?;
        let mut ranges = Vec::with_capacity(components);
        for component in 0..components {
            let number = |index: usize| -> Result<f64> {
                values
                    .get(index)
                    .map(|v| document.resolve(v))
                    .transpose()?
                    .and_then(Value::as_number)
                    .ok_or(Error::syntax("short image Decode"))
            };
            ranges.push([number(2 * component)?, number(2 * component + 1)?]);
        }
        return Ok(ranges);
    }
    let max = f64::from((1_u32 << bits) - 1);
    Ok((0..components)
        .map(|component| match space {
            Some(ColorSpace::Indexed(_)) => [0.0, max],
            Some(ColorSpace::Calibrated(cie)) => cie.lab_range().map_or([0.0, 1.0], |r| match component {
                0 => [0.0, 100.0],
                1 => [r[0], r[1]],
                _ => [r[2], r[3]],
            }),
            Some(ColorSpace::Icc(icc)) => icc.range.get(component).copied().unwrap_or([0.0, 1.0]),
            _ => [0.0, 1.0],
        })
        .collect())
}

/// `Matte` (11.6.5.3): one value per component of the base space, for a
/// soft mask with the image's dimensions.
fn matte(
    document: &Document,
    value: &Value,
    space: Option<&Arc<ColorSpace>>,
    source: &ImageSource,
    mask: &Image,
) -> Result<Components> {
    let count = space.map_or(0, |s| match s.as_ref() {
        ColorSpace::Indexed(indexed) => indexed.base.components(),
        s => s.components(),
    });
    let array = document
        .resolve(value)?
        .as_array()
        .filter(|a| a.len() == count)
        .ok_or(Error::syntax("invalid image Matte"))?;
    if mask.source.width() != source.width() || mask.source.height() != source.height() {
        return Err(Error::syntax("Matte mask dimensions differ from image"));
    }
    let values = array
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("invalid Matte component"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Components::from_slice(&values))
}

/// A colour-key `Mask` (8.9.6.4): an inclusive range of raw sample values
/// per component.
fn color_key(document: &Document, value: &Value, components: usize, bits: u8) -> Result<ImageMask> {
    let ranges = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("invalid image Mask"))?;
    if ranges.len() != components * 2 {
        return Err(Error::syntax("invalid colour key length"));
    }
    let max = u16::MAX >> (16 - u32::from(bits.min(16)));
    let mut keys = Vec::with_capacity(components);
    for pair in ranges.as_chunks::<2>().0 {
        let sample = |v: &Value| -> Result<u16> {
            u16::try_from(
                document
                    .resolve(v)?
                    .as_integer()
                    .ok_or(Error::syntax("invalid colour key"))?,
            )
            .ok()
            .filter(|&v| v <= max)
            .ok_or(Error::syntax("colour key out of range"))
        };
        let (low, high) = (sample(&pair[0])?, sample(&pair[1])?);
        if low > high {
            return Err(Error::syntax("reversed colour key range"));
        }
        keys.push([low, high]);
    }
    Ok(ImageMask::ColorKey(keys.into()))
}
