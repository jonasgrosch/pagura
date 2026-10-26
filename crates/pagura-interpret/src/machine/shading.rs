//! `sh` (8.7.4.3): a shading painted over the clip, leaving the current
//! path and colour unchanged. Devices without paint parse no shading.

use super::form::resource;
use super::{Machine, Scope};
use crate::device::{Device, MarkState, ShadingMark};
use crate::model::Shading;
use crate::resources::shading;
use pagura_core::{Dictionary, Name, Value};
use pagura_document::source::ReadResult;
use std::sync::Arc;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    pub(super) fn shading_operator(
        &mut self,
        name: &Name,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> ReadResult<()> {
        if !self.needs().paint {
            return Ok(());
        }
        let document = self.document();
        let value = resource(document, resources, b"Shading", name.as_bytes())?;
        let shading = self.read_shading(value, resources)?;
        let Some(emission) = self.admit(scope)? else {
            return Ok(());
        };
        let composite = self.fill_state(scope);
        Ok(self.device.shading(&ShadingMark {
            shading: &shading,
            transform: scope.state.ctm,
            state: MarkState {
                clip: scope.state.clip.as_ref(),
                composite: &composite,
                position: self.position,
                hidden: emission.hidden(),
            },
        })?)
    }

    /// Reads a shading whose colour space names refer to `resources`.
    pub(super) fn read_shading(
        &mut self,
        value: &Value,
        resources: &Dictionary,
    ) -> ReadResult<Arc<Shading>> {
        shading::read(&mut self.session.colors, resources, value).map(Arc::new)
    }
}
