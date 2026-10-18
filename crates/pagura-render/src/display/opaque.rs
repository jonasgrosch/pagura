//! The opaque renderer: source-over painting of solid Normal marks straight into
//! the 8-bit tile, without group surfaces (11.3.3, 11.4.5).

use super::{Command, DisplayList, GlyphCache, GlyphRun, Mark, PathCommand, contextual};
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Meter, Result};
use pagura_raster::{
    FillRule, Mask, Paint, Pixmap, Rasterizer, Transform, adjusted_stroke_outline, stroke_outline,
};
use std::sync::Arc;

impl DisplayList {
    pub(super) fn render_opaque(&self, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        let device = self.device_transform(options);
        let mut target =
            Pixmap::new_with_limits(tile.width, tile.height, options.background, self.limits)?;
        // The crop is a geometric clip, including fractional final rows/columns.
        let crop = crate::device::rectangle(self.crop);
        let crop = Mask::from_path_with_limits(
            tile.width,
            tile.height,
            &crop,
            device,
            FillRule::NonZero,
            self.limits,
            (tile.x, tile.y),
        )?;
        let mut raster = Rasterizer::default();
        let mut glyph_cache = GlyphCache::new(self.limits.decoded_bytes);
        // Function evaluation work shared by the page's colours and transfers.
        let mut work = 0;
        // Glyph placements share one geometry per outline, so each charges
        // its scan here; the compositor charges the same to its raster work.
        let mut glyphs = Meter::new(self.limits.raster_work, "glyph raster work exceeded");
        let mut last_clip = None;
        let mut mask = crop.clone();
        let mut stack = vec![self.commands.iter()];
        while let Some(iter) = stack.last_mut() {
            let Some(command) = iter.next() else {
                stack.pop();
                continue;
            };
            let original = command;
            let command = match command {
                Command::Path(command) => command,
                Command::Glyphs(run) => &run.paint,
                Command::Group(group) => {
                    if stack.len() >= self.limits.depth {
                        return Err(Error::limit("group nesting exceeded"));
                    }
                    stack.push(group.commands.iter());
                    continue;
                }
                Command::Shading(_) | Command::Image(_) => {
                    return Err(Error::invalid_argument("shading in opaque renderer"));
                }
            };
            let identity = command.clip.as_ref().map(Arc::as_ptr);
            if identity != last_clip {
                mask = crop.clone();
                let mut clip = command.clip.as_deref();
                let mut chain = Vec::new();
                while let Some(node) = clip {
                    chain.push(node);
                    clip = node.parent.as_deref();
                }
                for node in chain.into_iter().rev() {
                    let next = Mask::from_path_with_limits(
                        tile.width,
                        tile.height,
                        &node.path,
                        device,
                        node.rule,
                        self.limits,
                        (tile.x, tile.y),
                    )?;
                    mask.intersect(&next)?;
                }
                last_clip = identity;
            }
            let paint = command
                .color
                .to_rgba(
                    command.transfer.as_ref(),
                    command.composite.conversion.intent,
                    &mut work,
                )
                .map(Paint::Solid)
                .map_err(|e| contextual(e, command.position))?;
            let mut out = Opaque {
                target: &mut target,
                device,
                mask: &mask,
                raster: &mut raster,
                limits: &self.limits,
                glyphs: &mut glyphs,
            };
            let result = if let Command::Glyphs(run) = original {
                run.rasterize(&mut out, &mut glyph_cache, paint)
            } else {
                command.rasterize(&mut out, paint)
            };
            result.map_err(|e| contextual(e, command.position))?;
        }
        Ok(target)
    }
}

impl GlyphRun {
    fn rasterize(&self, out: &mut Opaque<'_>, cache: &mut GlyphCache, paint: Paint) -> Result<()> {
        let Opaque {
            target,
            device,
            mask,
            raster,
            limits,
            glyphs,
        } = out;
        let (device, limits) = (*device, *limits);
        for (index, &placement) in self.glyphs.iter().enumerate() {
            let geometry = cache
                .geometry(self, placement, device, limits, raster)
                .and_then(|geometry| glyphs.charge(geometry.vertices()).map(|()| geometry))
                .map_err(|error| contextual(error, self.position(index)))?;
            if cache
                .retained_bytes()
                .saturating_add(geometry.retained_bytes())
                .saturating_add(raster.retained_bytes())
                > limits.decoded_bytes
            {
                cache.clear();
                raster.clear();
            }
            raster
                .fill_glyph(target, geometry, placement.origin * device, &paint, mask)
                .map_err(|e| contextual(e, self.position(index)))?;
            if cache
                .retained_bytes()
                .saturating_add(raster.retained_bytes())
                > limits.decoded_bytes
            {
                cache.clear();
                raster.clear();
            }
        }
        Ok(())
    }
}

/// Where the opaque renderer paints one command: the tile, its device
/// transform and clip mask, the shared rasterizer and the page's limits.
struct Opaque<'a> {
    target: &'a mut Pixmap,
    device: Transform,
    mask: &'a Mask,
    raster: &'a mut Rasterizer,
    limits: &'a Limits,
    /// Outline vertices scanned by glyph placements across the render.
    glyphs: &'a mut Meter,
}

impl PathCommand {
    fn rasterize(&self, out: &mut Opaque<'_>, paint: Paint) -> Result<()> {
        let Opaque {
            target,
            device,
            mask,
            raster,
            limits,
            ..
        } = out;
        let device = *device;
        match &self.mark {
            Mark::Fill(rule) => {
                raster.fill_path(target, Arc::clone(&self.path), device, *rule, &paint, mask)?;
            }
            Mark::Stroke {
                pen: stroke,
                ctm,
                adjust,
            } => {
                let inverse = ctm.inverse().ok_or(Error::unsupported(
                    crate::features::SINGULAR_STROKE,
                    "singular stroke transform is not implemented",
                ))?;
                let path = self.path.transformed(inverse);
                let outline = if *adjust {
                    adjusted_stroke_outline
                } else {
                    stroke_outline
                };
                let (path, transform) = outline(&path, ctm.then(device), stroke, limits)?;
                raster.fill_path(
                    target,
                    Arc::new(path),
                    transform,
                    FillRule::NonZero,
                    &paint,
                    mask,
                )?;
            }
        }
        Ok(())
    }
}
