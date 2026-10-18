//! Shading marks and shading patterns sampled per device pixel (8.7.4.5).

use super::{Context, Frame, Model, Source, Surface, runs};
use crate::display::PathCommand;
use pagura_core::{Error, Result};
use pagura_raster::{FillRule, Point, Transform};
use std::sync::Arc;

impl Context {
    // Keep sampling and painting out of the command dispatch.
    #[inline(never)]
    pub(super) fn shading(
        &mut self,
        command: &crate::display::ShadingCommand,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        let transform = command.transform.then(self.device);
        let inverse = transform.inverse().ok_or(Error::unsupported(
            crate::features::SHADING_TRANSFORM,
            "singular shading transform",
        ))?;
        let clip = self.clip(command.clip.as_ref(), target.tile)?;
        let clip = if let Some(path) = &command.shading.bbox_path {
            let Some(mask) = clip.clipped_path(path, transform, FillRule::NonZero, self.limits)?
            else {
                return Ok(());
            };
            Arc::new(mask)
        } else {
            clip
        };
        let rate = if command.shading.antialias { 2 } else { 1 };
        self.charge(
            clip.coverage()
                .len()
                .checked_mul(rate * rate)
                .ok_or(Error::limit("shading sample count overflow"))?,
        )?;
        let soft = command
            .composite
            .mask
            .as_ref()
            .map(|mask| self.soft_mask(mask, target.tile, frame.model, frame.depth + 1))
            .transpose()?;
        let soft = soft.as_deref().map(Vec::as_slice);
        for run in runs(&clip) {
            let run = run?;
            let y = run.y;
            for (x, coverage) in run.pixels() {
                let source =
                    self.shading_sample(command, inverse, (x, y), frame.model, rate, None)?;
                let Some(mut source) = source else { continue };
                let factor = f64::from(coverage) / 255.0;
                source.alpha *= factor;
                source.shape *= factor;
                self.transfer_mark(
                    command.transfer.as_ref(),
                    &command.composite,
                    (x, y),
                    source,
                    true,
                )?;
                target.composite((x, y), source, &command.composite, soft, frame)?;
            }
        }
        Ok(())
    }
    pub(super) fn shading_sample(
        &mut self,
        command: &crate::display::ShadingCommand,
        inverse: Transform,
        coordinate: (u32, u32),
        model: Model,
        rate: usize,
        boundary: Option<f64>,
    ) -> Result<Option<Source>> {
        let background = boundary
            .and(command.shading.background.as_ref())
            .map(|values| {
                self.source(
                    &command.shading.space,
                    values,
                    model,
                    &command.composite,
                    false,
                )
            })
            .transpose()?;
        let certified = command.shading.bbox_path.as_ref().is_some_and(|path| {
            path.fill_region(FillRule::NonZero)
                .is_some_and(|region| region != pagura_raster::FillRegion::Ordinary)
        });
        let mut values = crate::color::Components::zeros(command.shading.space.components());
        let mut color = [0.0; 4];
        let mut covered = 0;
        let divisor =
            f64::from(u32::try_from(rate).map_err(|_| Error::limit("shading rate exceeds u32"))?);
        for y in 0..rate {
            for x in 0..rate {
                let point = Point::new(
                    f64::from(coordinate.0) + (x as f64 + 0.5) / divisor,
                    f64::from(coordinate.1) + (y as f64 + 0.5) / divisor,
                ) * inverse;
                let inside = certified
                    || command.shading.bbox.is_none_or(|b| {
                        point.x >= b.x0()
                            && point.x <= b.x1()
                            && point.y >= b.y0()
                            && point.y <= b.y1()
                    });
                let hit = (!command.background || inside)
                    && command.shading.sample(
                        point,
                        &mut values,
                        &mut self.function_work,
                        &mut self.work,
                    )?;
                if !hit {
                    if command.background {
                        if let Some(background) = command.shading.background {
                            values = background;
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                let sample = self.source(
                    &command.shading.space,
                    &values,
                    model,
                    &command.composite,
                    false,
                )?;
                for (i, out) in color.iter_mut().enumerate() {
                    *out += if let (Some(coverage), Some(background)) = (boundary, background) {
                        coverage * sample.color[i] + (1.0 - coverage) * background.color[i]
                    } else {
                        sample.color[i]
                    };
                }
                covered += 1;
            }
        }
        if covered == 0 {
            return Ok(None);
        }
        let shape = f64::from(covered) / (divisor * divisor);
        let mut source = self.sampled_source(&command.shading.space, model, &command.composite);
        source.color = color.map(|v| v / f64::from(covered));
        source.alpha = shape;
        source.shape = shape;
        Ok(Some(source))
    }
}

pub(super) fn pattern_source(p: &PathCommand) -> Option<crate::display::ShadingCommand> {
    if let crate::color::Color::Pattern {
        selected: Some(pattern),
        ..
    } = &p.color
    {
        let crate::pattern::Kind::Shading(shading) = &pattern.kind else {
            return None;
        };
        Some(crate::display::ShadingCommand {
            shading: Arc::clone(shading),
            position: p.position,
            transform: pattern.transform,
            clip: None,
            composite: p.composite.clone(),
            transfer: None,
            background: true,
        })
    } else {
        None
    }
}
