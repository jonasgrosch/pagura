//! Image resources and inline content framing (8.9).
use super::{Arc, Compiler, Dictionary, Error, ObjectId, Result, Value, resource};
use crate::{
    color::{self, Space},
    display::{Command, ImageCommand},
};
use pagura_core::{
    Feature, Name,
    syntax::{Parser, is_whitespace},
};
use pagura_image::{DecodedImage, ImageKind, JpxColor, MaskKind, Samples};

pub(crate) const STENCIL_PATTERN: Feature = Feature::new("image.stencil-pattern");
pub(crate) const EMBEDDED_COLOR: Feature = Feature::new("image.embedded-color");

#[derive(Debug)]
pub(crate) struct Image {
    pub(crate) decoded: DecodedImage,
    pub(crate) space: Option<Space>,
    pub(crate) mask: Option<Arc<Self>>,
    pub(crate) matte: Option<Vec<f64>>,
    pub(crate) decode: Vec<(f64, f64)>,
    pub(crate) intent: Option<crate::icc::Intent>,
}
impl Compiler<'_> {
    pub(super) fn xobject(&mut self, values: &[Value], resources: &Dictionary) -> Result<()> {
        let name = values
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("XObject name expected"))?;
        let value = resource(self.document, resources, b"XObject", name.as_bytes())?;
        let id = value
            .as_reference()
            .ok_or(Error::syntax("XObject must be a stream reference"))?;
        let object = self.document.object(id)?;
        let dict = object
            .dictionary()
            .ok_or(Error::syntax("XObject has no dictionary"))?;
        if dict.is_name(b"Subtype", b"Image") {
            let stencil = dict
                .get(b"ImageMask")
                .map(|v| self.document.resolve(v))
                .transpose()?
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if self.uncolored_glyph() && !stencil {
                return Ok(());
            }

            let visible = dict
                .get(b"OC")
                .map(|v| self.visibility.visible(self.document, v))
                .transpose()?
                .unwrap_or(true);
            self.extract_graphic_object(id, !visible)?;
            let image = self.image_resource(id, resources, &mut Vec::new())?;
            if self.visible && visible {
                self.image_mark(image)?;
            }
            Ok(())
        } else {
            self.form(values, resources)
        }
    }

    fn image_resource(
        &self,
        id: ObjectId,
        resources: &Dictionary,
        stack: &mut Vec<ObjectId>,
    ) -> Result<Arc<Image>> {
        if stack.contains(&id) {
            return Err(Error::cycle("recursive image mask").in_object(id));
        }
        if stack.len() >= self.document.limits().depth {
            return Err(Error::limit("image mask nesting exceeded"));
        }
        let key = (id, self.resource_identity(resources)?);
        if let Some(image) = self.state.text.cache.borrow().images.get(&key) {
            return Ok(Arc::clone(image));
        }
        stack.push(id);
        let object = self.document.object(id)?;
        let decoded = pagura_image::decode_xobject(self.document, object)?;
        let image = self
            .prepare_image(
                decoded,
                object
                    .dictionary()
                    .ok_or(Error::syntax("image dictionary missing"))?,
                resources,
                stack,
            )
            .map_err(|e| e.in_object(id))?;
        stack.pop();
        self.state
            .text
            .cache
            .borrow_mut()
            .images
            .insert(key, Arc::clone(&image));
        Ok(image)
    }

    fn prepare_image(
        &self,
        decoded: DecodedImage,
        dictionary: &Dictionary,
        resources: &Dictionary,
        stack: &mut Vec<ObjectId>,
    ) -> Result<Arc<Image>> {
        if decoded.components() == 0 || decoded.components() > 32 {
            return Err(Error::limit(
                "image component count exceeds painter capacity",
            ));
        }
        let space = self.image_space(&decoded, resources)?;
        if let Some(space) = &space
            && (matches!(space, Space::Pattern(_))
                || space.components() != usize::from(decoded.components()))
        {
            return Err(Error::syntax("image colour space component mismatch"));
        }
        let count = usize::try_from(u64::from(decoded.width()) * u64::from(decoded.height()))
            .map_err(|_| Error::limit("image pixel count exceeds usize"))?;
        let mask = match *decoded.mask() {
            MaskKind::Soft(id) | MaskKind::Stencil(id) => {
                Some(self.image_resource(id, resources, stack)?)
            }
            _ => None,
        };
        let matte = if let MaskKind::Soft(id) = *decoded.mask() {
            self.document
                .object(id)?
                .dictionary()
                .and_then(|d| d.get(b"Matte"))
                .map(|value| {
                    let array = self
                        .document
                        .resolve(value)?
                        .as_array()
                        .filter(|a| {
                            a.len()
                                == space.as_ref().map_or(0, |s| match s {
                                    Space::Indexed { base, .. } => base.components(),
                                    _ => s.components(),
                                })
                        })
                        .ok_or(Error::syntax("invalid image Matte"))?;
                    if mask.as_ref().is_none_or(|m| {
                        m.decoded.width() != decoded.width()
                            || m.decoded.height() != decoded.height()
                    }) {
                        return Err(Error::syntax("Matte mask dimensions differ from image"));
                    }
                    array
                        .iter()
                        .map(|v| {
                            self.document
                                .resolve(v)?
                                .as_number()
                                .ok_or(Error::syntax("invalid Matte component"))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
        } else {
            None
        };
        let decode = decode_ranges(&decoded, space.as_ref())?;
        let intent = self.image_intent(dictionary, space.as_ref())?;
        if decoded.samples().len()
            != count
                .checked_mul(usize::from(decoded.components()))
                .ok_or(Error::limit("image sample count overflow"))?
        {
            return Err(Error::syntax("image sample count mismatch"));
        }
        let mut bytes = sample_bytes(decoded.samples()).saturating_add(size_of::<Image>());
        if let MaskKind::Embedded(alpha) = decoded.mask() {
            if alpha.samples().len() != count {
                return Err(Error::syntax("image opacity sample count mismatch"));
            }
            bytes = bytes.saturating_add(sample_bytes(alpha.samples()));
        }
        self.paint_charge(bytes, 1)?;
        Ok(Arc::new(Image {
            decoded,
            space,
            mask,
            matte,
            decode,
            intent,
        }))
    }

    fn image_intent(
        &self,
        dictionary: &Dictionary,
        space: Option<&Space>,
    ) -> Result<Option<crate::icc::Intent>> {
        // Table 87: image masks keep the current nonstroking colour's intent.
        if space.is_none() {
            return Ok(None);
        }
        dictionary
            .get(b"Intent")
            .map(|value| self.document.resolve(value))
            .transpose()?
            .filter(|value| !value.is_null()) // Null entries are absent (7.3.9).
            .map(|value| {
                value
                    .as_name()
                    .map(|name| crate::icc::Intent::from_name(name.as_bytes()))
                    .ok_or(Error::syntax("image Intent must be a name"))
            })
            .transpose()
    }

    fn image_space(&self, decoded: &DecodedImage, resources: &Dictionary) -> Result<Option<Space>> {
        let context = crate::icc::ColorContext::new(self.document, &self.compilation.colors);
        let space = match decoded.kind() {
            ImageKind::Color { color_space } => {
                Some(color::blending(&context, resources, color_space)?)
            }
            ImageKind::EmbeddedColor { color } => {
                let name: &[u8] = match (color, decoded.components()) {
                    (JpxColor::Enumerated(16), 3) => b"DeviceRGB",
                    (JpxColor::Enumerated(17), 1) => b"DeviceGray",
                    _ => {
                        return Err(Error::unsupported(
                            EMBEDDED_COLOR,
                            "embedded JPX colour interpretation is not implemented",
                        ));
                    }
                };
                Some(color::named(&context, resources, name)?)
            }
            ImageKind::Stencil { .. } => None,
            _ => return Err(Error::syntax("image has no colour space")),
        };
        Ok(space)
    }

    fn image_mark(&mut self, image: Arc<Image>) -> Result<()> {
        if self.uncolored_glyph() && image.space.is_some() {
            return Ok(());
        }
        self.marks = self
            .marks
            .checked_add(1)
            .filter(|&n| n <= self.document.limits().display_commands)
            .ok_or(Error::limit("image display list budget exceeded"))?;
        let mut composite = self.state.composite.clone();
        if let Some(intent) = image.intent {
            composite.conversion.intent = intent;
        }
        // Image-local masks override the graphics-state soft mask (11.6.4.3).
        if !matches!(image.decoded.mask(), MaskKind::None) {
            composite.mask = None;
        }
        self.list.commands.push(Command::Image(ImageCommand {
            image,
            transform: self.state.ctm,
            clip: self.state.clip.clone(),
            stencil: self.state.fill.clone(),
            composite,
            transfer: self.state.transfer.clone(),
            position: self.position,
        }));
        self.paint_charge(0, 0)
    }

    fn inline_framing(
        &self,
        dictionary: &Dictionary,
        resources: &Dictionary,
    ) -> Result<Dictionary> {
        // Supply the actual colour-space definition for unfiltered size framing.
        let mut framing = dictionary.clone();
        if let Some(value) = dictionary
            .get(b"CS")
            .or_else(|| dictionary.get(b"ColorSpace"))
            && let Some(name) = value.as_name().filter(|n| {
                !matches!(
                    n.as_bytes(),
                    b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
                )
            })
        {
            let space = resource(self.document, resources, b"ColorSpace", name.as_bytes())?;
            let space = self.document.resolve(space)?;
            // Framing has no document resolver. ICC profiles are indirect;
            // supply only their channel count through an equivalent device
            // name here. Decoding and painting retain the real colour space.
            let framing_space = if space.as_array().is_some_and(|a| {
                a.first()
                    .and_then(Value::as_name)
                    .is_some_and(|n| n == b"ICCBased")
            }) {
                let context =
                    crate::icc::ColorContext::new(self.document, &self.compilation.colors);
                let count = color::blending(&context, resources, space)?.components();
                Value::name(match count {
                    1 => b"DeviceGray",
                    3 => b"DeviceRGB",
                    4 => b"DeviceCMYK",
                    _ => return Err(Error::syntax("invalid ICC component count")),
                })
            } else {
                space.clone()
            };
            framing.insert(Name::new(b"ColorSpace"), framing_space);
            framing.remove(b"CS");
        }
        Ok(framing)
    }

    pub(super) fn inline_image(
        &mut self,
        parser: &mut Parser<'_>,
        resources: &Dictionary,
    ) -> Result<()> {
        if !self.operands.is_empty() {
            return Err(Error::syntax("operands before BI"));
        }
        self.glyph_operator(b"BI")?;
        let mut dictionary = Dictionary::new();
        loop {
            self.operations = self
                .operations
                .checked_add(1)
                .filter(|&n| n <= self.document.limits().operations)
                .ok_or(Error::limit("inline image dictionary budget exceeded"))?;
            if parser.peek() == Some(b'I') {
                if parser.word()? != b"ID" {
                    return Err(Error::syntax("inline image requires ID"));
                }
                break;
            }
            let key = parser
                .parse()?
                .as_name()
                .cloned()
                .ok_or(Error::syntax("inline image key must be name"))?;
            let value = parser.parse()?;
            super::no_references(&value)?;
            if dictionary.contains_key(key.as_bytes()) {
                return Err(Error::syntax("duplicate inline image key"));
            }
            dictionary.insert(key, value);
        }
        let offset = parser.offset();
        let byte = *parser
            .data()
            .get(offset)
            .filter(|&&b| is_whitespace(b))
            .ok_or(Error::syntax("ID requires whitespace"))?;
        let offset = offset
            + if byte == b'\r' && parser.data().get(offset + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        let data = parser
            .data()
            .get(offset..)
            .ok_or(Error::syntax("missing inline image data"))?;
        let framing = self.inline_framing(&dictionary, resources)?;
        let length = pagura_image::inline_data_length(&framing, data, &self.document.limits())?;
        parser.seek(
            offset
                .checked_add(length)
                .ok_or(Error::limit("inline offset overflow"))?,
        )?;
        if parser
            .data()
            .get(parser.offset())
            .is_none_or(|&b| !is_whitespace(b))
            || parser.word()? != b"EI"
        {
            return Err(Error::syntax("inline image requires EI"));
        }
        let stencil = dictionary
            .get(b"ImageMask")
            .or_else(|| dictionary.get(b"IM"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if self.uncolored_glyph() && !stencil {
            return Ok(());
        }
        let image = pagura_image::decode_inline(
            self.document,
            resources,
            &dictionary,
            data.get(..length)
                .ok_or(Error::syntax("truncated inline data"))?,
        )?;
        let image = self.prepare_image(image, &dictionary, resources, &mut Vec::new())?;
        if self.visible {
            self.image_mark(image)?;
        }
        Ok(())
    }
}
fn sample_bytes(samples: &Samples) -> usize {
    samples
        .len()
        .saturating_mul(if matches!(samples, Samples::Bytes(_)) {
            1
        } else {
            2
        })
}
pub(crate) fn max_sample(image: &DecodedImage, component: usize) -> Result<u16> {
    let bits = image
        .component_bits(component)
        .filter(|b| (1..=16).contains(b))
        .ok_or(Error::syntax("invalid image component precision"))?;
    Ok(u16::MAX >> (16 - bits))
}
impl Image {
    pub(crate) fn components(&self, pixel: usize, values: &mut [f64; 32]) -> Result<()> {
        let n = usize::from(self.decoded.components());
        for (component, out) in values.iter_mut().enumerate().take(n) {
            let sample = self
                .decoded
                .samples()
                .get(
                    pixel
                        .checked_mul(n)
                        .and_then(|i| i.checked_add(component))
                        .ok_or(Error::limit("image sample offset overflow"))?,
                )
                .ok_or(Error::syntax("missing image sample"))?;
            let (low, high) = *self
                .decode
                .get(component)
                .ok_or(Error::syntax("missing Decode component"))?;
            *out = low
                + f64::from(sample) / f64::from(max_sample(&self.decoded, component)?)
                    * (high - low);
        }
        Ok(())
    }
}

fn decode_ranges(decoded: &DecodedImage, space: Option<&Space>) -> Result<Vec<(f64, f64)>> {
    let mut decode = Vec::with_capacity(usize::from(decoded.components()));
    for component in 0..usize::from(decoded.components()) {
        let range = if let Some(values) = decoded.decode() {
            (
                *values
                    .get(2 * component)
                    .ok_or(Error::syntax("short image Decode"))?,
                *values
                    .get(2 * component + 1)
                    .ok_or(Error::syntax("short image Decode"))?,
            )
        } else if matches!(space, Some(Space::Indexed { .. })) {
            (0.0, f64::from(max_sample(decoded, component)?))
        } else if let Some(space @ (Space::Cie(_) | Space::Icc { .. })) = space {
            let low = space.decode_lookup(&vec![0; usize::from(decoded.components())]);
            let high = space.decode_lookup(&vec![255; usize::from(decoded.components())]);
            (
                *low.get(component)
                    .ok_or(Error::syntax("missing CIE Decode"))?,
                *high
                    .get(component)
                    .ok_or(Error::syntax("missing CIE Decode"))?,
            )
        } else {
            (0.0, 1.0)
        };
        decode.push(range);
    }
    Ok(decode)
}
