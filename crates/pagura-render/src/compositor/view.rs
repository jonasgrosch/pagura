//! Screen annotation fitting about Rect's upper-left anchor (12.5.3).
use super::{Context, Frame, Surface};
use crate::transparency::Group;
use pagura_core::{Error, Result};
use pagura_raster::Transform;

impl Context {
    pub(super) fn view_group(
        &mut self,
        group: &Group,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        let device = self.device;
        if let Some(view) = group.attributes.view {
            let anchor = view.anchor * device;
            let angle = if view.no_rotate {
                -device.b.atan2(device.a)
            } else {
                0.0
            };
            let scale = if view.no_zoom {
                self.view_unit / device.a.hypot(device.b)
            } else {
                1.0
            };
            let (sin, cos) = angle.sin_cos();
            self.device = device
                .then(Transform::translate(-anchor.x, -anchor.y))
                .then(Transform::new(
                    scale * cos,
                    scale * sin,
                    -scale * sin,
                    scale * cos,
                    anchor.x,
                    anchor.y,
                ));
            if !self.device.is_finite() {
                return Err(Error::limit("annotation view transform overflow"));
            }
            self.clip = None;
            self.discard_mask();
        }
        let result = self.group(group, target, frame);
        if group.attributes.view.is_some() {
            self.device = device;
            self.clip = None;
            self.discard_mask();
        }
        result
    }
}
