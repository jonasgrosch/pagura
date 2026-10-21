//! Colour operators (8.6.8, Table 73). Devices without paint resolve no
//! colour space.

use super::state::Ink;
use super::{Machine, Scope};
use crate::device::Device;
use crate::repairs;
use pagura_color::{ColorSpace, Components};
use pagura_content::{ColorOperands, Op};
use pagura_core::{Dictionary, Error, Result};
use std::sync::Arc;

/// Which colour an operator sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Fill,
    Stroke,
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    pub(super) fn color(
        &mut self,
        op: Op<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        if !self.needs().paint {
            return Ok(());
        }
        let target = match op {
            Op::StrokeColorSpace(_)
            | Op::StrokeColor(_)
            | Op::StrokeColorN(_)
            | Op::StrokeGray(_)
            | Op::StrokeRgb(_)
            | Op::StrokeCmyk(_) => Target::Stroke,
            _ => Target::Fill,
        };
        let ink = match op {
            Op::StrokeColorSpace(name) | Op::FillColorSpace(name) => {
                Ink::initial(&self.session.colors.named(resources, name.as_bytes())?)
            }
            Op::StrokeGray(gray) | Op::FillGray(gray) => {
                self.device_color(resources, b"DeviceGray", &[gray])?
            }
            Op::StrokeRgb(rgb) | Op::FillRgb(rgb) => {
                self.device_color(resources, b"DeviceRGB", &rgb)?
            }
            Op::StrokeCmyk(cmyk) | Op::FillCmyk(cmyk) => {
                self.device_color(resources, b"DeviceCMYK", &cmyk)?
            }
            Op::StrokeColor(operands) | Op::FillColor(operands) => {
                let space = current(scope, target)?;
                self.simple_operator(&space, &operands)?;
                set(&space, &operands)?
            }
            Op::StrokeColorN(operands) | Op::FillColorN(operands) => {
                let space = current(scope, target)?;
                if let ColorSpace::Pattern(base) = space.as_ref() {
                    let paint = self.select_pattern(base.as_ref(), &operands, resources, scope)?;
                    Ink { space, paint }
                } else {
                    set(&space, &operands)?
                }
            }
            _ => return Err(Error::syntax("invalid colour operator")),
        };
        match target {
            Target::Fill => scope.state.fill = Some(ink),
            Target::Stroke => scope.state.stroke_ink = Some(ink),
        }
        Ok(())
    }

    fn device_color(&mut self, resources: &Dictionary, name: &[u8], values: &[f64]) -> Result<Ink> {
        let space = self.session.colors.named(resources, name)?;
        if values.len() != space.components() {
            return Err(Error::syntax("wrong colour component count"));
        }
        Ok(Ink::solid(&space, Components::from_slice(values)))
    }

    /// Table 73: `SC` and `sc` do not select Separation, DeviceN or ICCBased
    /// colours; operands that fit are a reported repair.
    fn simple_operator(&self, space: &ColorSpace, operands: &ColorOperands<'_>) -> Result<()> {
        if matches!(
            space,
            ColorSpace::Separation(_) | ColorSpace::DeviceN(_) | ColorSpace::Icc(_)
        ) {
            if operands.len() != space.components() || operands.pattern.is_some() {
                return Err(Error::syntax("colour space requires SCN or scn"));
            }
            self.repair(repairs::COLOR_OPERATOR, "colour space requires SCN or scn")?;
        }
        Ok(())
    }
}

/// The space of the colour `target` names.
fn current(scope: &Scope, target: Target) -> Result<Arc<ColorSpace>> {
    let ink = match target {
        Target::Fill => scope.state.fill.as_ref(),
        Target::Stroke => scope.state.stroke_ink.as_ref(),
    };
    ink.map(|ink| Arc::clone(&ink.space))
        .ok_or(Error::syntax("colour space is not set"))
}

/// A colour in a non-pattern `space` from numeric operands.
pub(super) fn set(space: &Arc<ColorSpace>, operands: &ColorOperands<'_>) -> Result<Ink> {
    if matches!(space.as_ref(), ColorSpace::Pattern(_)) {
        return Err(Error::syntax("pattern requires SCN or scn"));
    }
    if operands.len() != space.components() {
        return Err(Error::syntax("wrong colour component count"));
    }
    if operands.pattern.is_some() {
        return Err(Error::syntax("colour component must be a number"));
    }
    Ok(Ink::solid(
        space,
        Components::from_slice(&operands.components),
    ))
}
