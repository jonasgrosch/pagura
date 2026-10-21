//! Soft-mask source groups captured in the coordinate system at gs (11.6.5.1).

use super::{Compiler, Dictionary, Error, Result, Value};
use crate::display::Command;
use crate::functions::Function;
use crate::transparency::{Composite, MaskMethod, Model, SoftMask};
use std::sync::Arc;

impl Compiler<'_> {
    pub(super) fn soft_mask(
        &mut self,
        value: &Value,
        resources: &Dictionary,
    ) -> Result<Option<Arc<SoftMask>>> {
        let value = self.document.resolve(value)?;
        if value.as_name().is_some_and(|n| n == b"None") {
            return Ok(None);
        }
        let d = self.document.resolve_dictionary(value)?;
        if let Some(kind) = d.get(b"Type")
            && self
                .document
                .resolve(kind)?
                .as_name()
                .is_none_or(|n| n != b"Mask")
        {
            return Err(Error::syntax("invalid soft-mask Type"));
        }
        let method = match d.get_name(b"S").map(pagura_core::Name::as_bytes) {
            Some(b"Alpha") => MaskMethod::Alpha,
            Some(b"Luminosity") => MaskMethod::Luminosity,
            _ => return Err(Error::syntax("invalid soft-mask subtype")),
        };
        let id = d
            .get(b"G")
            .and_then(Value::as_reference)
            .ok_or(Error::syntax("soft mask requires a group stream"))?;
        let mut child = self.detached();
        child.state.clip = None;
        child.state.clip_depth = 0;
        child.state.composite = Composite {
            alpha: 1.0,
            blend: crate::blend::BlendMode::Normal,
            mask: None,
            ..child.state.composite.clone()
        };
        child.state.stroke_alpha = 1.0;
        child.form_object(id, resources)?;
        self.operations = child.operations;
        self.marks = child.marks;
        let mut commands = child.list.commands.into_iter();
        let Some(Command::Group(group)) = commands.next() else {
            return Err(Error::syntax("soft-mask G must be a transparency group").in_object(id));
        };
        if commands.next().is_some() {
            return Err(Error::syntax("soft mask has invalid group structure"));
        }
        if matches!(method, MaskMethod::Luminosity) && group.attributes.model.is_none() {
            return Err(
                Error::syntax("luminosity mask requires a group colour space").in_object(id),
            );
        }
        let model = group.attributes.model.unwrap_or(Model::Rgb);
        let backdrop = if matches!(method, MaskMethod::Luminosity) {
            d.get(b"BC")
                .map(|v| model.values(&crate::numbers::list(self.document, v, f64::MAX)?))
                .transpose()?
                .unwrap_or(model.black())
        } else {
            [0.0; 4]
        };
        let transfer = d
            .get(b"TR")
            .map(|v| {
                if self
                    .document
                    .resolve(v)?
                    .as_name()
                    .is_some_and(|n| n == b"Identity")
                {
                    Ok(None)
                } else {
                    Function::parse(self.document, v, 1, 1).map(Some)
                }
            })
            .transpose()?
            .flatten();
        Ok(Some(Arc::new(SoftMask {
            group,
            method,
            backdrop,
            transfer,
        })))
    }
}
