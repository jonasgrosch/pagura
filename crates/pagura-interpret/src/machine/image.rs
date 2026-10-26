//! Images (8.9): XObjects and inline images described for painting without
//! decoding their samples, and their object occurrences for structure.

mod describe;

use super::{Machine, Scope};
use crate::device::{Device, ImageMark, MarkState, Marked};
use crate::model::Image;
use pagura_core::{Dictionary, Error, Name, ObjectId, Position, Result, Value};
use pagura_document::source::ReadResult;
use pagura_image::{ImageId, ImageSource};
use std::collections::HashMap;
use std::sync::Arc;

/// Image descriptions read in one interpretation, by image and the colour
/// space resources they were resolved in: the only resources describing
/// an image reads (named and `Default` spaces, 8.6.5.6).
#[derive(Debug, Default)]
pub(crate) struct Images {
    entries: HashMap<(ObjectId, usize), Arc<Image>>,
    /// The `ColorSpace` entries of the resources met.
    spaces: Vec<Option<Value>>,
}

impl Images {
    /// A number identifying the colour space resources of `resources`
    /// among those met.
    fn spaces(&mut self, resources: &Dictionary) -> Result<usize> {
        let spaces = resources.get(b"ColorSpace");
        if let Some(index) = self.spaces.iter().position(|s| s.as_ref() == spaces) {
            return Ok(index);
        }
        if self.spaces.len() >= 256 {
            return Err(Error::limit("image resource cache budget exceeded"));
        }
        self.spaces.push(spaces.cloned());
        Ok(self.spaces.len() - 1)
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
    ) -> ReadResult<()> {
        let document = self.document();
        let stencil = dictionary
            .get(b"ImageMask")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if scope.uncolored_glyph() && !stencil {
            return Ok(());
        }
        let visible = dictionary
            .get(b"OC")
            .map(|v| self.session.visibility.visible(document, v))
            .transpose()?
            .unwrap_or(true);
        if self.structure(scope) {
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
    ) -> ReadResult<Arc<Image>> {
        if stack.contains(&id) {
            return Err(Error::cycle("recursive image mask").in_object(id).into());
        }
        if stack.len() >= self.session.limits.depth {
            return Err(Error::limit("image mask nesting exceeded").into());
        }
        let key = (id, self.session.images.spaces(resources)?);
        if let Some(image) = self.session.images.entries.get(&key) {
            return Ok(Arc::clone(image));
        }
        stack.push(id);
        let document = self.document();
        let object = document.object(id)?;
        let source = ImageSource::from_xobject(document, object).map_err(|e| e.in_object(id))?;
        let image = self
            .describe(source, resources, stack)
            .map_err(|e| e.in_object(id))?;
        stack.pop();
        self.session.images.entries.insert(key, Arc::clone(&image));
        Ok(image)
    }

    /// Emits an image or stencil mask mark.
    fn image_mark(&mut self, image: &Arc<Image>, scope: &mut Scope) -> Result<()> {
        if scope.uncolored_glyph() && image.space.is_some() {
            return Ok(());
        }
        let Some(emission) = self.admit(scope)? else {
            return Ok(());
        };
        let base = self.fill_state(scope);
        let composite = if self.needs().paint && (image.intent.is_some() || has_mask(image)) {
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

    /// `BI … ID … EI` (8.9.7) with its `data`.
    pub(super) fn inline_image(
        &mut self,
        dictionary: &Dictionary,
        data: &[u8],
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> ReadResult<()> {
        let stencil = dictionary
            .get(b"ImageMask")
            .or_else(|| dictionary.get(b"IM"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if (scope.uncolored_glyph() && !stencil) || !self.needs().images {
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
            dictionary,
            data,
            ImageId::Inline { stream, offset },
        )
        .map_err(|error| error.at(Position::Buffer(0)))?;
        let image = self.describe(source, resources, &mut Vec::new())?;
        Ok(self.image_mark(&image, scope)?)
    }
}

/// The dictionary that frames an inline image's data (8.9.7): a named
/// colour space is replaced by its definition, and an ICCBased one by the
/// device space of its stream's `N`, without resolving the space.
pub(super) fn framing(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    resources: &Dictionary,
) -> ReadResult<Dictionary> {
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
