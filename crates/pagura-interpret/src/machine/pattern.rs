//! Pattern colours (8.7.3, 8.7.4.2): shading patterns, and tiling cells
//! defined once per pattern, matrix and starting state in the state of the
//! content that uses them (8.7.3.1). Uncoloured cells ignore colour
//! operators (8.6.8).

use super::path::add_clip;
use super::state::Ink;
use super::{Machine, Scope, Text};
use crate::device::{Device, TilingInfo};
use crate::model::{Paint, PatternId, ShadingPaint, TilingColors, TilingPaint};
use crate::repairs;
use crate::resources::pattern::{self, Kind, Tiling};
use pagura_color::ColorSpace;
use pagura_content::{ColorOperands, Content, Op};
use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_raster::{FillRule, Path, Transform};
use std::sync::Arc;

/// A coloured tiling cell: the pattern, its matrix and the starting state
/// of the content using it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TilingKey {
    object: ObjectId,
    matrix: [u64; 6],
    serial: u64,
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// `SCN`/`scn` in a Pattern space: selects and defines the pattern.
    pub(super) fn select_pattern(
        &mut self,
        base: Option<&Arc<ColorSpace>>,
        operands: &ColorOperands,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<Arc<Paint>> {
        let Some(name) = operands.pattern.as_ref() else {
            return Err(if operands.components.is_empty() {
                Error::syntax("missing pattern name")
            } else {
                Error::syntax("pattern name expected")
            });
        };
        let document = self.document();
        let pattern = pattern::read(document, resources, name.as_bytes(), scope.initial_ctm)?;
        let uncolored = match &pattern.kind {
            Kind::Tiling(t) if t.colors == TilingColors::Uncolored => {
                let space = base.ok_or(Error::syntax(
                    "uncoloured pattern requires underlying space",
                ))?;
                let without_name = ColorOperands {
                    components: operands.components.clone(),
                    pattern: None,
                };
                Some(super::color::set(space, &without_name)?)
            }
            _ if !operands.components.is_empty() => {
                if base.is_none_or(|s| s.components() != operands.components.len()) {
                    return Err(Error::syntax("invalid unused pattern colour operands"));
                }
                self.repair(
                    repairs::PATTERN_UNDERLYING,
                    "coloured pattern uses intrinsic colours; extra scn operands ignored (8.7.3.2, Table 73)",
                )?;
                None
            }
            _ => None,
        };
        match pattern.kind {
            Kind::Shading(value) => {
                let shading = self.read_shading(value, resources)?;
                Ok(Arc::new(Paint::Shading(ShadingPaint {
                    shading,
                    matrix: pattern.matrix,
                })))
            }
            Kind::Tiling(tiling) => {
                let id = self.tiling(&tiling, pattern.matrix, uncolored.as_ref(), scope)?;
                let color = uncolored.and_then(|ink| match Arc::unwrap_or_clone(ink.paint) {
                    Paint::Solid(color) => Some(color),
                    _ => None,
                });
                Ok(Arc::new(Paint::Tiling(TilingPaint { pattern: id, color })))
            }
        }
    }

    /// Defines a tiling cell, or finds the coloured cell defined for the
    /// same pattern, matrix and starting state.
    fn tiling(
        &mut self,
        tiling: &Tiling<'_>,
        matrix: Transform,
        uncolored: Option<&Ink>,
        scope: &Scope,
    ) -> Result<PatternId> {
        if scope.forms.contains(&tiling.id) {
            return Err(Error::cycle("recursive tiling pattern"));
        }
        if scope.forms.len() >= self.session.limits.depth {
            return Err(Error::limit("pattern nesting exceeded"));
        }
        let serial = scope.initial.as_ref().map_or(0, |(_, serial)| *serial);
        let key = TilingKey {
            object: tiling.id,
            matrix: [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
                .map(f64::to_bits),
            serial,
        };
        if uncolored.is_none()
            && let Some(&id) = self.session.patterns.get(&key)
        {
            return Ok(id);
        }
        let id = PatternId(self.session.next_pattern);
        self.session.next_pattern = self
            .session
            .next_pattern
            .checked_add(1)
            .ok_or(Error::limit("tiling pattern count exceeded"))?;
        self.define_cell(tiling, matrix, uncolored, scope, id)
            .map_err(|e| e.in_object(tiling.id))?;
        if uncolored.is_none() {
            self.session.patterns.insert(key, id);
        }
        Ok(id)
    }

    fn define_cell(
        &mut self,
        tiling: &Tiling<'_>,
        matrix: Transform,
        uncolored: Option<&Ink>,
        scope: &Scope,
        id: PatternId,
    ) -> Result<()> {
        let document = self.document();
        let object = document.object(tiling.id)?;
        let mut cell = scope.child(Text::Paint);
        if let Some((initial, _)) = &scope.initial {
            cell.state = initial.as_ref().clone();
        }
        cell.state.set_ctm(Transform::IDENTITY);
        cell.state.clip = None;
        cell.state.clip_depth = 0;
        cell.state.reset_composite();
        cell.initial_ctm = Transform::IDENTITY;
        cell.procedure = None;
        cell.stencil = uncolored.is_some();
        cell.forms.push(tiling.id);
        if let Some(ink) = uncolored {
            cell.state.fill = Some(ink.clone());
            cell.state.stroke_ink = Some(ink.clone());
        }
        let bounds = Arc::new(Path::rect(tiling.bbox));
        add_clip(&mut cell, bounds, FillRule::NonZero, self.session.limits.depth)?;
        let content = Content::from_streams(
            [(tiling.id, document.stream_data(object)?)],
            &self.session.limits,
        )?;
        let outer = self.begin_definition();
        let result = (|| {
            self.device.begin_tiling(&TilingInfo {
                id,
                bbox: tiling.bbox,
                step: tiling.step,
                matrix,
                colors: tiling.colors,
                object: Some(tiling.id),
            })?;
            self.run(&content, tiling.resources, &mut cell)?;
            self.device.end_tiling()
        })();
        self.end_definition(outer);
        result
    }

    /// Whether an uncoloured tiling cell ignores `op` (8.6.8): colour
    /// operators, `ri`, `sh`, and images that are not stencil masks. The
    /// restriction holds in nested forms too.
    pub(super) fn stencil_ignores(
        &self,
        op: &Op,
        resources: &Dictionary,
        scope: &Scope,
    ) -> Result<bool> {
        if !scope.stencil {
            return Ok(false);
        }
        Ok(match op {
            Op::StrokeColorSpace(_)
            | Op::FillColorSpace(_)
            | Op::StrokeColor(_)
            | Op::FillColor(_)
            | Op::StrokeColorN(_)
            | Op::FillColorN(_)
            | Op::StrokeGray(_)
            | Op::FillGray(_)
            | Op::StrokeRgb(_)
            | Op::FillRgb(_)
            | Op::StrokeCmyk(_)
            | Op::FillCmyk(_)
            | Op::RenderingIntent(_)
            | Op::Shading(_) => true,
            Op::XObject(name) => {
                let document = self.document();
                let value = super::form::resource(document, resources, b"XObject", name.as_bytes())?;
                let d = document.resolve_dictionary(value)?;
                d.is_name(b"Subtype", b"Image")
                    && d.get(b"ImageMask")
                        .map(|v| document.resolve(v))
                        .transpose()?
                        .and_then(Value::as_bool)
                        != Some(true)
            }
            Op::InlineImage(dictionary) => {
                dictionary
                    .get(b"IM")
                    .or_else(|| dictionary.get(b"ImageMask"))
                    .and_then(Value::as_bool)
                    != Some(true)
            }
            _ => false,
        })
    }
}
