//! Images (8.9): XObjects and inline images described for painting without
//! decoding their samples, and their object occurrences for structure.

mod describe;

use super::{Machine, Scope, Text};
use crate::device::{Device, ImageMark, Marked, MarkState};
use crate::model::Image;
use pagura_content::Reader;
use pagura_core::{Dictionary, Error, Name, ObjectId, Position, Result, Value};
use pagura_image::{ImageId, ImageSource};
use std::collections::HashMap;
use std::sync::Arc;

/// Image descriptions read in one interpretation, by image and the
/// resources its colour space was resolved in.
#[derive(Debug, Default)]
pub(crate) struct Images {
    entries: HashMap<(ObjectId, usize), Arc<Image>>,
    /// The resources dictionaries met, kept so their addresses identify
    /// them for the whole interpretation.
    resources: Vec<Dictionary>,
}

impl Images {
    /// A number identifying `resources` among those met.
    fn resources(&mut self, resources: &Dictionary) -> Result<usize> {
        if let Some(index) = self.resources.iter().position(|r| r == resources) {
            return Ok(index);
        }
        if self.resources.len() >= 256 {
            return Err(Error::limit("image resource cache budget exceeded"));
        }
        self.resources.push(resources.clone());
        Ok(self.resources.len() - 1)
    }
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// `Do` on an image XObject.
    pub(super) fn image_xobject(
        &mut self,
        id: ObjectId,
        dictionary: &Dictionary,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let document = self.document();
        let stencil = dictionary
            .get(b"ImageMask")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if self.uncolored_glyph(scope) && !stencil {
            return Ok(());
        }
        let visible = dictionary
            .get(b"OC")
            .map(|v| self.session.visibility.visible(document, v))
            .transpose()?
            .unwrap_or(true);
        if scope.role == Text::Page && self.needs().text && self.needs().structure {
            self.device.marked(&Marked::ObjectBegin {
                object: id,
                hidden: !visible || !scope.visible,
            })?;
            self.device.marked(&Marked::ObjectEnd)?;
        }
        if !self.needs().images {
            return Ok(());
        }
        let image = self.xobject_image(id, resources, &mut Vec::new())?;
        if visible {
            self.image_mark(&image, scope)?;
        }
        Ok(())
    }

    /// The description of image XObject `id`, read once per resources.
    pub(crate) fn xobject_image(
        &mut self,
        id: ObjectId,
        resources: &Dictionary,
        stack: &mut Vec<ObjectId>,
    ) -> Result<Arc<Image>> {
        if stack.contains(&id) {
            return Err(Error::cycle("recursive image mask").in_object(id));
        }
        if stack.len() >= self.session.limits.depth {
            return Err(Error::limit("image mask nesting exceeded"));
        }
        let key = (id, self.session.images.resources(resources)?);
        if let Some(image) = self.session.images.entries.get(&key) {
            return Ok(Arc::clone(image));
        }
        stack.push(id);
        let document = self.document();
        let object = document.object(id)?;
        let source = ImageSource::from_xobject(document, object).map_err(|e| e.in_object(id))?;
        let dictionary = object
            .dictionary()
            .ok_or(Error::syntax("image dictionary missing"))?;
        let image = self
            .describe(source, dictionary, resources, stack)
            .map_err(|e| e.in_object(id))?;
        stack.pop();
        self.session.images.entries.insert(key, Arc::clone(&image));
        Ok(image)
    }

    /// Emits an image or stencil mask mark.
    fn image_mark(&mut self, image: &Arc<Image>, scope: &mut Scope) -> Result<()> {
        if self.uncolored_glyph(scope) && image.space.is_some() {
            return Ok(());
        }
        let Some(emission) = self.admit(scope)? else {
            return Ok(());
        };
        let base = scope.state.fill_composite();
        let composite = if image.intent.is_some() || has_mask(image) {
            let mut composite = base.as_ref().clone();
            if let Some(intent) = image.intent {
                composite.conversion.intent = intent;
            }
            // Image-local masks override the graphics state's soft mask
            // (11.6.4.3).
            if has_mask(image) {
                composite.soft_mask = None;
            }
            Arc::new(composite)
        } else {
            base
        };
        let stencil = if image.space.is_none() {
            self.fill_paint(scope)
        } else {
            None
        };
        self.device.image(&ImageMark {
            image,
            transform: scope.state.ctm,
            stencil: stencil.as_ref(),
            state: MarkState {
                clip: scope.state.clip.as_ref(),
                composite: &composite,
                position: self.position,
                hidden: emission.hidden(),
            },
        })
    }

    /// `BI … ID … EI` (8.9.7).
    pub(super) fn inline_image(
        &mut self,
        dictionary: Dictionary,
        reader: &mut Reader<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let framing = if self.needs().paint {
            self.inline_framing(&dictionary, resources)?
        } else {
            light_framing(self.document(), &dictionary, resources)?
        };
        let data = reader.inline_image_data(|data| pagura_image::inline_data_length(&framing, data))?;
        let stencil = dictionary
            .get(b"ImageMask")
            .or_else(|| dictionary.get(b"IM"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if (self.uncolored_glyph(scope) && !stencil) || !self.needs().images {
            return Ok(());
        }
        let (stream, offset) = match self.position {
            Position::Stream { object, offset } => (Some(object), offset),
            Position::Buffer(offset) => (None, offset),
            _ => (None, 0),
        };
        let source = ImageSource::from_inline(
            self.document(),
            resources,
            &dictionary,
            data,
            ImageId::Inline { stream, offset },
        )
        .map_err(|error| error.at(Position::Buffer(0)))?;
        let image = self.describe(source, &dictionary, resources, &mut Vec::new())?;
        self.image_mark(&image, scope)
    }

    /// Reads past an inline image a stencil cell ignores (8.6.8): only its
    /// header and encoded length are examined.
    pub(super) fn skip_inline_image(
        &mut self,
        dictionary: &Dictionary,
        reader: &mut Reader<'_>,
        resources: &Dictionary,
    ) -> Result<()> {
        let framing = light_framing(self.document(), dictionary, resources)?;
        reader.inline_image_data(|data| pagura_image::inline_data_length(&framing, data))?;
        Ok(())
    }

    /// The dictionary inline image framing needs: a named colour space is
    /// replaced by its definition, and an ICCBased one by the device space
    /// of its component count.
    fn inline_framing(&mut self, dictionary: &Dictionary, resources: &Dictionary) -> Result<Dictionary> {
        let document = self.document();
        let mut framing = dictionary.clone();
        let Some(name) = named_space(dictionary) else {
            return Ok(framing);
        };
        let value = super::form::resource(document, resources, b"ColorSpace", name.as_bytes())?;
        let value = document.resolve(value)?;
        let definition = if is_icc(value) {
            let count = self.session.colors.resolve(resources, value)?.components();
            device_name(count)?
        } else {
            value.clone()
        };
        framing.insert(Name::new(b"ColorSpace"), definition);
        framing.remove(b"CS");
        Ok(framing)
    }
}

/// Framing without colour-space resolution: an ICCBased space's `N` gives
/// its component count.
fn light_framing(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    resources: &Dictionary,
) -> Result<Dictionary> {
    let mut framing = dictionary.clone();
    let Some(name) = named_space(dictionary) else {
        return Ok(framing);
    };
    let value = super::form::resource(document, resources, b"ColorSpace", name.as_bytes())?;
    let value = document.resolve(value)?;
    let definition = if is_icc(value) {
        let count = value
            .as_array()
            .and_then(|a| a.get(1))
            .and_then(Value::as_reference)
            .map(|id| document.object(id))
            .transpose()?
            .and_then(|object| object.dictionary())
            .and_then(|d| d.get(b"N"))
            .map(|n| document.resolve(n))
            .transpose()?
            .and_then(Value::as_integer)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::syntax("invalid ICC component count"))?;
        device_name(count)?
    } else {
        value.clone()
    };
    framing.insert(Name::new(b"ColorSpace"), definition);
    framing.remove(b"CS");
    Ok(framing)
}

/// The inline image's colour space name, unless it is a device space.
fn named_space(dictionary: &Dictionary) -> Option<&Name> {
    dictionary
        .get(b"CS")
        .or_else(|| dictionary.get(b"ColorSpace"))
        .and_then(Value::as_name)
        .filter(|n| {
            !matches!(
                n.as_bytes(),
                b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
            )
        })
}

fn is_icc(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        a.first()
            .and_then(Value::as_name)
            .is_some_and(|n| n == b"ICCBased")
    })
}

fn device_name(count: usize) -> Result<Value> {
    Ok(Value::name(match count {
        1 => b"DeviceGray",
        3 => b"DeviceRGB",
        4 => b"DeviceCMYK",
        _ => return Err(Error::syntax("invalid ICC component count")),
    }))
}

fn has_mask(image: &Image) -> bool {
    !matches!(image.mask, crate::model::ImageMask::None)
}
