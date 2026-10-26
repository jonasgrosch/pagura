//! Soft masks (11.6.5.2): the mask's group interpreted in the coordinate
//! system and state at `gs`, numbered in definition order.

use super::form::{FormBody, HiddenForm};
use super::{Machine, Scope, Text};
use crate::device::{Device, SoftMaskInfo};
use crate::model::{MaskId, MaskKind};
use crate::resources::{function, group, numbers};
use pagura_color::Components;
use pagura_core::{Dictionary, Error, Value};
use pagura_document::source::ReadResult;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// An `SMask` entry: `None`, or a mask dictionary whose group is
    /// interpreted now.
    pub(super) fn soft_mask(
        &mut self,
        value: &Value,
        resources: &Dictionary,
        scope: &Scope,
    ) -> ReadResult<Option<MaskId>> {
        let document = self.document();
        let value = document.resolve(value)?;
        if value.as_name().is_some_and(|n| n == b"None") {
            return Ok(None);
        }
        let d = document.resolve_dictionary(value)?;
        if let Some(kind) = d.get(b"Type")
            && document
                .resolve(kind)?
                .as_name()
                .is_none_or(|n| n != b"Mask")
        {
            return Err(Error::syntax("invalid soft-mask Type").into());
        }
        let kind = match d.get_name(b"S").map(pagura_core::Name::as_bytes) {
            Some(b"Alpha") => MaskKind::Alpha,
            Some(b"Luminosity") => MaskKind::Luminosity,
            _ => return Err(Error::syntax("invalid soft-mask subtype").into()),
        };
        let id = d
            .get(b"G")
            .and_then(Value::as_reference)
            .ok_or(Error::syntax("soft mask requires a group stream"))?;
        self.mask_group(id, kind, d, resources, scope).map(Some)
    }

    fn mask_group(
        &mut self,
        id: pagura_core::ObjectId,
        kind: MaskKind,
        d: &Dictionary,
        resources: &Dictionary,
        scope: &Scope,
    ) -> ReadResult<MaskId> {
        let document = self.document();
        let mut parent = scope.child(Text::Paint);
        parent.state.clip = None;
        parent.state.clip_depth = 0;
        parent.state.start_group();
        let object = document.object(id)?;
        let not_group = || Error::syntax("soft-mask G must be a transparency group").in_object(id);
        let Some(form) = self.prepare_form(
            FormBody::Stored(object),
            resources,
            &parent,
            HiddenForm::Skip,
        )?
        else {
            return Err(not_group().into());
        };
        let attributes =
            group::attributes(&mut self.session.colors, form.dictionary, form.resources)?
                .ok_or_else(not_group)?;
        if kind == MaskKind::Luminosity && attributes.space.is_none() {
            return Err(
                Error::syntax("luminosity mask requires a group colour space")
                    .in_object(id)
                    .into(),
            );
        }
        let backdrop = match (kind, attributes.space, d.get(b"BC")) {
            (MaskKind::Luminosity, Some(space), Some(value)) => {
                let values = numbers::list(document, value, f64::MAX)?;
                if values.len() != space.components() {
                    return Err(Error::syntax("wrong group backdrop component count").into());
                }
                Some(Components::from_slice(&values))
            }
            _ => None,
        };
        let transfer = d
            .get(b"TR")
            .map(|v| {
                if document
                    .resolve(v)?
                    .as_name()
                    .is_some_and(|n| n == b"Identity")
                {
                    Ok(None)
                } else {
                    function::scalar(document, v).map(Some)
                }
            })
            .transpose()?
            .flatten();
        let mask = self.next_mask()?;
        let mut child = form.scope;
        child.state.start_group();
        let outer = self.begin_definition();
        let result = (|| -> ReadResult<()> {
            self.device.begin_soft_mask(&SoftMaskInfo {
                id: mask,
                kind,
                group: attributes,
                bbox: form.bounds.as_ref(),
                backdrop: backdrop.as_ref(),
                transfer: transfer.as_ref(),
                position: self.position,
            })?;
            self.run(&form.content, form.resources, &mut child)?;
            Ok(self.device.end_soft_mask()?)
        })();
        self.end_definition(outer);
        result.map(|()| mask)
    }
}
