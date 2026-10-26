//! Colour operators (8.6.8, Table 73). Devices without paint resolve no
//! colour space.

use super::state::{Ink, SolidPaints};
use super::{Machine, Scope};
use crate::device::Device;
use crate::repairs;
use pagura_color::ColorSpace;
use pagura_content::{ColorOperands, Op};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::source::ReadResult;
use std::sync::Arc;

/// Which colour an operator sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Fill,
    Stroke,
}

/// The device colour families of `g`, `rg` and `k`.
#[derive(Debug, Clone, Copy)]
enum Family {
    Gray,
    Rgb,
    Cmyk,
}

impl Family {
    fn name(self) -> &'static [u8] {
        match self {
            Self::Gray => b"DeviceGray",
            Self::Rgb => b"DeviceRGB",
            Self::Cmyk => b"DeviceCMYK",
        }
    }
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    pub(super) fn color(
        &mut self,
        op: Op<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> ReadResult<()> {
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
                self.device_color(resources, Family::Gray, &[gray], scope)?
            }
            Op::StrokeRgb(rgb) | Op::FillRgb(rgb) => {
                self.device_color(resources, Family::Rgb, &rgb, scope)?
            }
            Op::StrokeCmyk(cmyk) | Op::FillCmyk(cmyk) => {
                self.device_color(resources, Family::Cmyk, &cmyk, scope)?
            }
            Op::StrokeColor(operands) | Op::FillColor(operands) => {
                let space = current(scope, target)?;
                self.simple_operator(&space, &operands)?;
                set(&space, &operands, &mut self.session.solids)?
            }
            Op::StrokeColorN(operands) | Op::FillColorN(operands) => {
                let space = current(scope, target)?;
                if let ColorSpace::Pattern(base) = space.as_ref() {
                    let paint = self.select_pattern(base.as_ref(), &operands, resources, scope)?;
                    Ink { space, paint }
                } else {
                    set(&space, &operands, &mut self.session.solids)?
                }
            }
            _ => return Err(Error::syntax("invalid colour operator").into()),
        };
        match target {
            Target::Fill => scope.state.fill = Some(ink),
            Target::Stroke => scope.state.stroke_ink = Some(ink),
        }
        Ok(())
    }

    /// A colour of the device space `family` sets (`g`, `rg`, `k`), which a
    /// default colour space of `resources` may replace (8.6.5.6). The space
    /// is resolved once per content sequence, whose resources are fixed.
    fn device_color(
        &mut self,
        resources: &Dictionary,
        family: Family,
        values: &[f64],
        scope: &Scope,
    ) -> ReadResult<Ink> {
        let serial = scope.initial.as_ref().map(|(_, serial)| *serial);
        let slot = family as usize;
        let cached = self
            .session
            .device_spaces
            .get(slot)
            .and_then(Option::as_ref)
            .filter(|(at, _)| Some(*at) == serial)
            .map(|(_, space)| Arc::clone(space));
        let space = if let Some(space) = cached {
            space
        } else {
            let space = self.session.colors.named(resources, family.name())?;
            if let (Some(serial), Some(entry)) = (serial, self.session.device_spaces.get_mut(slot))
            {
                *entry = Some((serial, Arc::clone(&space)));
            }
            space
        };
        if values.len() != space.components() {
            return Err(Error::syntax("wrong colour component count").into());
        }
        Ok(self.session.solids.ink(&space, values))
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
pub(super) fn set(
    space: &Arc<ColorSpace>,
    operands: &ColorOperands<'_>,
    solids: &mut SolidPaints,
) -> Result<Ink> {
    if matches!(space.as_ref(), ColorSpace::Pattern(_)) {
        return Err(Error::syntax("pattern requires SCN or scn"));
    }
    if operands.len() != space.components() {
        return Err(Error::syntax("wrong colour component count"));
    }
    if operands.pattern.is_some() {
        return Err(Error::syntax("colour component must be a number"));
    }
    Ok(solids.ink(space, &operands.components))
}
