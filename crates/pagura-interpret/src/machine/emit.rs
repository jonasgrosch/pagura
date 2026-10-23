//! Emitting marks and groups into the device: optional-content visibility,
//! implicit groups opened at their first visible mark, and the states marks
//! carry.

use super::{Machine, Scope};
use crate::device::{Device, FillMark, GroupInfo, MarkState, StrokeMark};
use crate::model::{Composite, GroupAttributes, GroupCause, Paint, ViewFit};
use pagura_core::{Position, Result};
use pagura_raster::{FillRule, Path};
use std::sync::Arc;

/// An implicit group (text knockout, a Type 3 glyph) that opens when its
/// content first paints a visible mark, so that empty ones never reach the
/// device.
#[derive(Debug, Clone)]
pub(crate) struct Lazy {
    cause: GroupCause,
    attributes: GroupAttributes,
    clip: Option<Arc<crate::model::Clip>>,
    position: Position,
    opened: bool,
}

/// How a mark reaches the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Emission {
    /// Visible.
    Visible,
    /// Hidden by optional content, for a device that needs structure.
    Hidden,
}

impl Emission {
    pub(crate) fn hidden(self) -> bool {
        self == Self::Hidden
    }
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// Whether a mark in `scope` reaches the device: visible marks first
    /// open pending implicit groups; hidden marks reach only devices that
    /// need structure.
    pub(crate) fn admit(&mut self, scope: &Scope) -> Result<Option<Emission>> {
        if scope.visible {
            self.open_lazy()?;
            Ok(Some(Emission::Visible))
        } else if self.needs().structure {
            Ok(Some(Emission::Hidden))
        } else {
            Ok(None)
        }
    }

    /// Opens every pending implicit group, outermost first.
    pub(crate) fn open_lazy(&mut self) -> Result<()> {
        // Pending groups are the innermost ones: when the innermost is open,
        // every group is.
        if self.lazy.last().is_none_or(|lazy| lazy.opened) {
            return Ok(());
        }
        let opaque = Arc::clone(&self.session.opaque);
        for lazy in self.lazy.iter_mut().filter(|lazy| !lazy.opened) {
            self.device.begin_group(&GroupInfo {
                cause: lazy.cause,
                attributes: lazy.attributes,
                bbox: None,
                view: None,
                state: MarkState {
                    clip: lazy.clip.as_ref(),
                    composite: &opaque,
                    position: lazy.position,
                    hidden: false,
                },
            })?;
            lazy.opened = true;
        }
        Ok(())
    }

    /// Starts an implicit group with `attributes`, opaque and Normal.
    pub(crate) fn push_lazy(
        &mut self,
        cause: GroupCause,
        attributes: GroupAttributes,
        scope: &Scope,
    ) {
        self.lazy.push(Lazy {
            cause,
            attributes,
            clip: scope.state.clip.clone(),
            position: self.position,
            opened: false,
        });
    }

    /// Ends the innermost implicit group; returns whether it was opened.
    pub(crate) fn pop_lazy(&mut self) -> Result<bool> {
        let opened = self.lazy.pop().is_some_and(|lazy| lazy.opened);
        if opened {
            self.device.end_group()?;
        }
        Ok(opened)
    }

    /// Starts the content of a soft mask or tiling cell, which belongs to no
    /// implicit group of the content around it.
    pub(crate) fn begin_definition(&mut self) -> Vec<Lazy> {
        std::mem::take(&mut self.lazy)
    }

    /// Ends what [`Self::begin_definition`] started.
    pub(crate) fn end_definition(&mut self, lazy: Vec<Lazy>) {
        self.lazy = lazy;
    }

    /// Emits a group around content that follows.
    pub(crate) fn begin_group(&mut self, group: &GroupStart<'_>, scope: &Scope) -> Result<()> {
        self.open_lazy()?;
        self.device.begin_group(&GroupInfo {
            cause: group.cause,
            attributes: group.attributes,
            bbox: group.bbox,
            view: group.view,
            state: MarkState {
                clip: scope.state.clip.as_ref(),
                composite: group.composite,
                position: self.position,
                hidden: false,
            },
        })
    }

    /// Emits a fill of `path` with the fill paint, or `composite` in place
    /// of the fill state's.
    pub(crate) fn emit_fill(
        &mut self,
        scope: &mut Scope,
        path: &Arc<Path>,
        rule: FillRule,
        composite: Option<Arc<Composite>>,
    ) -> Result<()> {
        let Some(emission) = self.admit(scope)? else {
            return Ok(());
        };
        let composite = composite.unwrap_or_else(|| self.fill_state(scope));
        let paint = self.fill_paint(scope);
        self.device.fill(&FillMark {
            path,
            rule,
            paint: paint.as_ref(),
            state: MarkState {
                clip: scope.state.clip.as_ref(),
                composite: &composite,
                position: self.position,
                hidden: emission.hidden(),
            },
        })
    }

    /// Emits a stroke of `path` with the stroke paint and pen.
    pub(crate) fn emit_stroke(
        &mut self,
        scope: &mut Scope,
        path: &Arc<Path>,
        composite: Option<Arc<Composite>>,
    ) -> Result<()> {
        let Some(emission) = self.admit(scope)? else {
            return Ok(());
        };
        let composite = composite.unwrap_or_else(|| self.stroke_state(scope));
        let pen = scope.state.pen();
        let paint = self.stroke_paint(scope);
        self.device.stroke(&StrokeMark {
            path,
            pen: &pen,
            paint: paint.as_ref(),
            state: MarkState {
                clip: scope.state.clip.as_ref(),
                composite: &composite,
                position: self.position,
                hidden: emission.hidden(),
            },
        })
    }

    /// The composite state of fills, glyphs, images and shadings: the
    /// graphics state's for devices that need paint, else the default.
    pub(crate) fn fill_state(&self, scope: &mut Scope) -> Arc<Composite> {
        if self.needs().paint {
            scope.state.fill_composite()
        } else {
            Arc::clone(&self.session.opaque)
        }
    }

    /// The composite state of strokes, as [`Self::fill_state`].
    pub(crate) fn stroke_state(&self, scope: &mut Scope) -> Arc<Composite> {
        if self.needs().paint {
            scope.state.stroke_composite()
        } else {
            Arc::clone(&self.session.opaque)
        }
    }

    /// The fill paint, for devices that need paint.
    pub(crate) fn fill_paint(&self, scope: &Scope) -> Option<Arc<Paint>> {
        if !self.needs().paint {
            return None;
        }
        scope.state.fill.as_ref().map(|ink| Arc::clone(&ink.paint))
    }

    /// The stroke paint, for devices that need paint.
    pub(crate) fn stroke_paint(&self, scope: &Scope) -> Option<Arc<Paint>> {
        if !self.needs().paint {
            return None;
        }
        scope
            .state
            .stroke_ink
            .as_ref()
            .map(|ink| Arc::clone(&ink.paint))
    }
}

/// A group to begin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GroupStart<'a> {
    pub(crate) cause: GroupCause,
    pub(crate) attributes: GroupAttributes,
    pub(crate) bbox: Option<&'a Arc<Path>>,
    pub(crate) view: Option<ViewFit>,
    pub(crate) composite: &'a Arc<Composite>,
}
