//! Floating-point group composition; quantization occurs at the output boundary.
//!
//! All intermediate channels, alpha and shape use f64 (11.7.2). Non-isolated
//! group colours have their initial backdrop removed before final composition
//! (11.4.4). Knockout replaces prior contributions according to shape, not alpha.

use crate::display::{Clip, Command, DisplayList, Mark, PathCommand};
use crate::transparency::{Composite, Group, MaskMethod, Model, SoftMask};
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Result};
use pagura_raster::{
    FillRule, Mask, Path, PathBuilder, Pixmap, Point, Segment, Transform, adjusted_stroke_outline,
    stroke_outline,
};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default)]
struct Pixel {
    color: [f64; 4],
    alpha: f64,
    group: f64,
    shape: f64,
}
impl Pixel {
    fn source(color: [f64; 4], alpha: f64) -> Self {
        Self {
            color: color.map(|v| v * alpha),
            alpha,
            ..Self::default()
        }
    }
    fn straight(self) -> [f64; 4] {
        if self.alpha > 0.0 {
            self.color.map(|v| (v / self.alpha).clamp(0.0, 1.0))
        } else {
            [0.0; 4]
        }
    }
    fn paint(
        &mut self,
        source: Source,
        state: &Composite,
        initial: Self,
        knockout: bool,
        model: Model,
    ) {
        let Source {
            color,
            alpha,
            shape,
        } = source;
        let base = if knockout { initial } else { *self };
        let opacity = if knockout && shape > 0.0 {
            alpha / shape
        } else {
            alpha
        };
        let blend = state
            .blend
            .apply(base.straight(), color, model.components());
        let value = [0, 1, 2, 3].map(|i| {
            (1.0 - opacity) * base.color[i]
                + opacity * ((1.0 - base.alpha) * color[i] + base.alpha * blend[i])
        });
        let value_alpha = opacity + base.alpha * (1.0 - opacity);
        if knockout {
            self.color = [0, 1, 2, 3].map(|i| (1.0 - shape) * self.color[i] + shape * value[i]);
            self.alpha = (1.0 - shape) * self.alpha + shape * value_alpha;
            self.group = (1.0 - shape) * self.group + alpha;
        } else {
            self.color = value;
            self.alpha = value_alpha;
            self.group = alpha + self.group * (1.0 - alpha);
        }
        self.shape = shape + self.shape * (1.0 - shape);
    }
}
#[derive(Debug, Clone, Copy)]
struct Source {
    color: [f64; 4],
    alpha: f64,
    shape: f64,
}
impl Source {
    fn apply(mut self, state: &Composite, mask: f64) -> Self {
        let factor = state.alpha * mask;
        self.alpha *= factor;
        if state.alpha_is_shape {
            self.shape *= factor;
        }
        self
    }
}

struct Surface {
    tile: Tile,
    pixels: Vec<Pixel>,
}
impl Surface {
    fn pixel(&self, x: u32, y: u32) -> Pixel {
        self.index(x, y)
            .and_then(|i| self.pixels.get(i))
            .copied()
            .unwrap_or_default()
    }
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        let (x, y) = (x.checked_sub(self.tile.x)?, y.checked_sub(self.tile.y)?);
        if x >= self.tile.width || y >= self.tile.height {
            return None;
        }
        usize::try_from(u64::from(y) * u64::from(self.tile.width) + u64::from(x)).ok()
    }
    fn bytes(&self) -> usize {
        self.pixels.len() * size_of::<Pixel>()
    }
}
#[derive(Clone, Copy)]
enum Backdrop<'a> {
    Transparent,
    Color([f64; 4]),
    Surface(&'a Surface),
}
impl Backdrop<'_> {
    fn pixel(self, x: u32, y: u32) -> Pixel {
        let mut p = match self {
            Self::Transparent => Pixel::default(),
            Self::Color(c) => Pixel::source(c, 1.0),
            Self::Surface(s) => s.pixel(x, y),
        };
        p.group = 0.0;
        p.shape = 0.0;
        p
    }
}

struct Context {
    limits: Limits,
    device: Transform,
    crop: Path,
    allocated: usize,
    work: usize,
    clip: Option<(usize, Tile, Arc<Mask>)>,
    mask: Option<CachedMask>,
    function_work: usize,
    soft_source: bool,
}
struct CachedMask {
    id: usize,
    tile: Tile,
    model: Model,
    values: Arc<Vec<f64>>,
}
impl Context {
    fn charge(&mut self, work: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(work)
            .filter(|&v| v <= self.limits.raster_work)
            .ok_or(Error::limit("compositing work budget exceeded"))?;
        Ok(())
    }
    fn surface(&mut self, tile: Tile, initial: Backdrop<'_>) -> Result<Surface> {
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .ok()
            .filter(|&v| v <= self.limits.pixels)
            .ok_or(Error::limit("group pixel budget exceeded"))?;
        let bytes = count
            .checked_mul(size_of::<Pixel>())
            .ok_or(Error::limit("group byte size overflow"))?;
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or(Error::limit("group memory budget exceeded"))?;
        self.charge(count)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("group allocation failed"))?;
        for y in tile.y..tile.y + tile.height {
            for x in tile.x..tile.x + tile.width {
                pixels.push(initial.pixel(x, y));
            }
        }
        Ok(Surface { tile, pixels })
    }
    fn clip(&mut self, clip: Option<&Arc<Clip>>, tile: Tile) -> Result<Arc<Mask>> {
        let id = clip.map_or(0, |c| Arc::as_ptr(c) as usize);
        if let Some((key, region, mask)) = &self.clip
            && *key == id
            && *region == tile
        {
            return Ok(Arc::clone(mask));
        }
        let mut viewport = PathBuilder::new();
        viewport.rect(
            f64::from(tile.x),
            f64::from(tile.y),
            f64::from(tile.width),
            f64::from(tile.height),
        );
        let viewport = viewport.finish();
        let (path, transform) = if self.soft_source {
            (&viewport, Transform::IDENTITY)
        } else {
            (&self.crop, self.device)
        };
        let mut mask = Mask::from_path_with_limits(
            tile.width,
            tile.height,
            path,
            transform,
            FillRule::NonZero,
            self.limits,
            (tile.x, tile.y),
        )?;
        let mut chain = Vec::new();
        let mut current = clip.map(AsRef::as_ref);
        while let Some(node) = current {
            chain.push(node);
            current = node.parent.as_deref();
        }
        for node in chain.into_iter().rev() {
            let next = Mask::from_path_with_limits(
                tile.width,
                tile.height,
                &node.path,
                self.device,
                node.rule,
                self.limits,
                (tile.x, tile.y),
            )?;
            mask.intersect(&next)?;
        }
        let mask = Arc::new(mask);
        self.clip = Some((id, tile, Arc::clone(&mask)));
        Ok(mask)
    }
    fn commands(
        &mut self,
        commands: &[Command],
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if depth >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        for command in commands {
            let result = match command {
                Command::Path(path) => self.path(path, target, initial, model, knockout, depth),
                Command::Shading(shading) => {
                    self.shading(shading, target, initial, model, knockout, depth)
                }
                Command::Group(group) => self.group(group, target, initial, model, knockout, depth),
            };
            result.map_err(|e| crate::display::contextual(e, command.position()))?;
        }
        Ok(())
    }
    fn path(
        &mut self,
        p: &PathCommand,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if matches!(
            &p.color,
            crate::color::Color::Pattern { selected: None, .. }
        ) {
            return Ok(());
        }
        if p.transfer.is_some() {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_TRANSFER,
                "transfer selection across transparent artwork is not implemented",
            ));
        }
        if model == Model::Cmyk && p.composite.overprint {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_OVERPRINT,
                "CMYK group overprint is not implemented",
            ));
        }
        let Some(mask) = self.path_mask(p, target.tile)? else {
            return Ok(());
        };
        let pattern = pattern_source(p);
        self.charge(mask.coverage().len())?;
        let color = if pattern.is_none() {
            Some(model.color(&p.color, &p.composite.conversion, &mut self.function_work)?)
        } else {
            None
        };
        let inverse = pattern
            .as_ref()
            .map(|p| {
                p.transform
                    .then(self.device)
                    .inverse()
                    .ok_or(Error::unsupported(
                        crate::features::SHADING_TRANSFORM,
                        "singular pattern transform",
                    ))
            })
            .transpose()?;
        let soft = p
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, target.tile, model, depth + 1))
            .transpose()?;
        let (ox, oy) = mask.origin();
        let width =
            usize::try_from(mask.width()).map_err(|_| Error::limit("mask width exceeds usize"))?;
        for (row, coverage) in mask.coverage().chunks_exact(width).enumerate() {
            let y = oy + u32::try_from(row).map_err(|_| Error::limit("mask row exceeds u32"))?;
            for (col, &coverage) in coverage.iter().enumerate().filter(|(_, v)| **v != 0) {
                let x =
                    ox + u32::try_from(col).map_err(|_| Error::limit("mask column exceeds u32"))?;
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("mask outside group"))?;
                let mask_value = soft
                    .as_ref()
                    .and_then(|m| m.get(index))
                    .copied()
                    .unwrap_or(1.0);
                let shape = f64::from(coverage) / 255.0;
                let source = if let Some(color) = color {
                    Source {
                        color,
                        alpha: shape,
                        shape,
                    }
                } else if let (Some(pattern), Some(inverse)) = (&pattern, inverse) {
                    let rate = if pattern.shading.antialias { 2 } else { 1 };
                    self.charge(rate * rate)?;
                    let Some(mut source) =
                        self.shading_sample(pattern, inverse, (x, y), model, rate)?
                    else {
                        continue;
                    };
                    source.alpha *= shape;
                    source.shape *= shape;
                    source
                } else {
                    return Err(Error::invalid_argument("missing pattern source"));
                };
                let source = source.apply(&p.composite, mask_value);
                let pixel = target
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("invalid group index"))?;
                pixel.paint(source, &p.composite, initial.pixel(x, y), knockout, model);
            }
        }
        Ok(())
    }
    fn path_mask(&mut self, p: &PathCommand, tile: Tile) -> Result<Option<Mask>> {
        let clip = self.clip(p.clip.as_ref(), tile)?;
        let mask = match &p.mark {
            Mark::Fill(rule) => clip.clipped_path(&p.path, self.device, *rule, self.limits)?,
            Mark::Stroke(stroke, ctm, adjust) => {
                let inverse = ctm.inverse().ok_or(Error::unsupported(
                    crate::features::SINGULAR_STROKE,
                    "singular stroke transform",
                ))?;
                let path = p.path.transformed(inverse);
                let outline = if *adjust {
                    adjusted_stroke_outline
                } else {
                    stroke_outline
                };
                let (path, transform) =
                    outline(&path, ctm.then(self.device), stroke, &self.limits)?;
                clip.clipped_path(&path, transform, FillRule::NonZero, self.limits)?
            }
        };
        let Some(mask) = mask else { return Ok(None) };
        if let crate::color::Color::Pattern {
            selected: Some(pattern),
            ..
        } = &p.color
            && pattern.shading.background.is_none()
            && let Some(bbox) = pattern.shading.bbox
        {
            let mut path = PathBuilder::new();
            path.rect(bbox.x0, bbox.y0, bbox.width(), bbox.height());
            let Some(clipped) = mask.clipped_path(
                &path.finish(),
                pattern.transform.then(self.device),
                FillRule::NonZero,
                self.limits,
            )?
            else {
                return Ok(None);
            };
            return Ok(Some(clipped));
        }
        Ok(Some(mask))
    }
    fn group(
        &mut self,
        group: &Group,
        parent: &mut Surface,
        parent_initial: Backdrop<'_>,
        parent_model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        let Some(tile) = group_region(group, parent.tile, self.device)? else {
            return Ok(());
        };
        let model = if group.attributes.isolated {
            group.attributes.model.unwrap_or(parent_model)
        } else {
            parent_model
        };
        let initial = if group.attributes.isolated {
            Backdrop::Transparent
        } else if knockout {
            parent_initial
        } else {
            Backdrop::Surface(parent)
        };
        let mut surface = self.surface(tile, initial)?;
        self.commands(
            &group.commands,
            &mut surface,
            initial,
            model,
            group.attributes.knockout,
            depth + 1,
        )?;
        let soft = group
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, parent.tile, parent_model, depth + 1))
            .transpose()?;
        self.charge(surface.pixels.len())?;
        let width =
            usize::try_from(tile.width).map_err(|_| Error::limit("group width exceeds usize"))?;
        // Remove the initial backdrop before mutating the parent surface. This
        // also prevents a prior result from becoming a later pixel's backdrop.
        for (row, pixels) in surface.pixels.chunks_exact_mut(width).enumerate() {
            let y =
                tile.y + u32::try_from(row).map_err(|_| Error::limit("group row exceeds u32"))?;
            for (column, pixel) in pixels.iter_mut().enumerate().filter(|(_, p)| p.shape > 0.0) {
                let x = tile.x
                    + u32::try_from(column)
                        .map_err(|_| Error::limit("group column exceeds u32"))?;
                let base = initial.pixel(x, y);
                let color = if pixel.group > 0.0 {
                    [0, 1, 2, 3].map(|i| {
                        ((pixel.color[i] - (1.0 - pixel.group) * base.color[i]) / pixel.group)
                            .clamp(0.0, 1.0)
                    })
                } else {
                    [0.0; 4]
                };
                pixel.color = if model == parent_model {
                    color
                } else {
                    parent_model.encode_rgb(
                        model.rgb(color),
                        &group.composite.conversion,
                        &mut self.function_work,
                    )?
                };
            }
        }
        for (row, pixels) in surface.pixels.chunks_exact(width).enumerate() {
            let y =
                tile.y + u32::try_from(row).map_err(|_| Error::limit("group row exceeds u32"))?;
            for (column, pixel) in pixels.iter().enumerate().filter(|(_, p)| p.shape > 0.0) {
                let x = tile.x
                    + u32::try_from(column)
                        .map_err(|_| Error::limit("group column exceeds u32"))?;
                let index = parent
                    .index(x, y)
                    .ok_or(Error::invalid_argument("group outside parent"))?;
                let mask = soft
                    .as_ref()
                    .and_then(|m| m.get(index))
                    .copied()
                    .unwrap_or(1.0);
                let source = Source {
                    color: pixel.color,
                    alpha: pixel.group,
                    shape: pixel.shape,
                }
                .apply(&group.composite, mask);
                parent
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("invalid parent index"))?
                    .paint(
                        source,
                        &group.composite,
                        parent_initial.pixel(x, y),
                        knockout,
                        parent_model,
                    );
            }
        }
        self.allocated -= surface.bytes();
        Ok(())
    }
    fn shading(
        &mut self,
        command: &crate::display::ShadingCommand,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if command.transfer.is_some() {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_TRANSFER,
                "shading transfer-region selection is not implemented",
            ));
        }
        let transform = command.transform.then(self.device);
        let inverse = transform.inverse().ok_or(Error::unsupported(
            crate::features::SHADING_TRANSFORM,
            "singular shading transform",
        ))?;
        let clip = self.clip(command.clip.as_ref(), target.tile)?;
        let clip = if let Some(bbox) = command.shading.bbox {
            let mut path = PathBuilder::new();
            path.rect(bbox.x0, bbox.y0, bbox.width(), bbox.height());
            let Some(mask) =
                clip.clipped_path(&path.finish(), transform, FillRule::NonZero, self.limits)?
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
            .map(|mask| self.soft_mask(mask, target.tile, model, depth + 1))
            .transpose()?;
        let width = usize::try_from(clip.width())
            .map_err(|_| Error::limit("shading width exceeds usize"))?;
        let origin = clip.origin();
        for (row, coverage) in clip.coverage().chunks_exact(width).enumerate() {
            let y = origin.1
                + u32::try_from(row).map_err(|_| Error::limit("shading row exceeds u32"))?;
            for (column, &coverage) in coverage.iter().enumerate().filter(|(_, v)| **v != 0) {
                let x = origin.0
                    + u32::try_from(column)
                        .map_err(|_| Error::limit("shading column exceeds u32"))?;
                let source = self.shading_sample(command, inverse, (x, y), model, rate)?;
                let Some(mut source) = source else { continue };
                let factor = f64::from(coverage) / 255.0;
                source.alpha *= factor;
                source.shape *= factor;
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("shading outside surface"))?;
                let mask = soft
                    .as_ref()
                    .and_then(|v| v.get(index))
                    .copied()
                    .unwrap_or(1.0);
                target
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("shading index outside surface"))?
                    .paint(
                        source.apply(&command.composite, mask),
                        &command.composite,
                        initial.pixel(x, y),
                        knockout,
                        model,
                    );
            }
        }
        Ok(())
    }
    fn shading_sample(
        &mut self,
        command: &crate::display::ShadingCommand,
        inverse: Transform,
        coordinate: (u32, u32),
        model: Model,
        rate: usize,
    ) -> Result<Option<Source>> {
        let mut values = [0.0; 32];
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
                let inside = command.shading.bbox.is_none_or(|b| {
                    point.x >= b.x0 && point.x <= b.x1 && point.y >= b.y0 && point.y <= b.y1
                });
                let hit = (!command.background || inside)
                    && command
                        .shading
                        .sample(point, &mut values, &mut self.function_work)?;
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
                let sample = model.source(
                    &command.shading.space,
                    &values[..command.shading.space.components()],
                    &command.composite.conversion,
                    &mut self.function_work,
                )?;
                for (out, sample) in color.iter_mut().zip(sample) {
                    *out += sample;
                }
                covered += 1;
            }
        }
        if covered == 0 {
            return Ok(None);
        }
        let shape = f64::from(covered) / (divisor * divisor);
        Ok(Some(Source {
            color: color.map(|v| v / f64::from(covered)),
            alpha: shape,
            shape,
        }))
    }
    fn discard_mask(&mut self) {
        if let Some(mask) = self.mask.take() {
            self.allocated -= mask.values.len() * size_of::<f64>();
        }
    }
    fn soft_mask(
        &mut self,
        mask: &Arc<SoftMask>,
        tile: Tile,
        parent_model: Model,
        depth: usize,
    ) -> Result<Arc<Vec<f64>>> {
        let id = Arc::as_ptr(mask) as usize;
        if let Some(cached) = &self.mask
            && cached.id == id
            && cached.tile == tile
            && cached.model == parent_model
        {
            return Ok(Arc::clone(&cached.values));
        }
        self.discard_mask();
        let model = mask.group.attributes.model.unwrap_or(parent_model);
        let initial =
            if matches!(mask.method, MaskMethod::Luminosity) && !mask.group.attributes.isolated {
                Backdrop::Color(mask.backdrop)
            } else {
                Backdrop::Transparent
            };
        let mut surface = group_region(&mask.group, tile, self.device)?
            .map(|region| self.surface(region, initial))
            .transpose()?;
        if let Some(surface) = &mut surface {
            // Mask G has its own BBox; the page crop is applied to the painted
            // object later, not to the mask source (11.6.5.1).
            let previous = std::mem::replace(&mut self.soft_source, true);
            self.clip = None;
            let result = self.commands(
                &mask.group.commands,
                surface,
                initial,
                model,
                mask.group.attributes.knockout,
                depth + 1,
            );
            self.soft_source = previous;
            self.clip = None;
            result?;
        }
        self.discard_mask();
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .map_err(|_| Error::limit("mask size exceeds usize"))?;
        let bytes = count
            .checked_mul(size_of::<f64>())
            .ok_or(Error::limit("mask bytes overflow"))?;
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or(Error::limit("soft-mask memory budget exceeded"))?;
        self.charge(count)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("soft-mask allocation failed"))?;
        let (mut last, mut mapped) = (f64::NAN, 0.0);
        for y in tile.y..tile.y + tile.height {
            for x in tile.x..tile.x + tile.width {
                let p = surface
                    .as_ref()
                    .and_then(|s| s.index(x, y).and_then(|i| s.pixels.get(i)))
                    .copied()
                    .unwrap_or_else(|| initial.pixel(x, y));
                let value = mask_value(mask, p, model).clamp(0.0, 1.0);
                if value != last {
                    mapped = if let Some(function) = &mask.transfer {
                        function
                            .evaluate_with_work(&[value], &mut self.function_work)?
                            .first()
                            .copied()
                            .ok_or(Error::syntax("mask transfer has no output"))?
                            .clamp(0.0, 1.0)
                    } else {
                        value
                    };
                    last = value;
                }
                values.push(mapped);
            }
        }
        if let Some(surface) = surface {
            self.allocated -= surface.bytes();
        }
        let values = Arc::new(values);
        self.mask = Some(CachedMask {
            id,
            tile,
            model: parent_model,
            values: Arc::clone(&values),
        });
        Ok(values)
    }
}

pub(crate) fn render(list: &DisplayList, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
    let mut crop = PathBuilder::new();
    crop.rect(
        list.crop.x0,
        list.crop.y0,
        list.crop.width(),
        list.crop.height(),
    );
    let mut context = Context {
        limits: list.limits,
        device: list.device_transform(options),
        crop: crop.finish(),
        allocated: 0,
        work: 0,
        clip: None,
        mask: None,
        function_work: 0,
        soft_source: false,
    };
    let mut page = context.surface(tile, Backdrop::Transparent)?;
    let model = list.group.model.unwrap_or(Model::Rgb);
    context.commands(
        &list.commands,
        &mut page,
        Backdrop::Transparent,
        model,
        list.group.knockout,
        0,
    )?;
    let mut output =
        Pixmap::new_with_limits(tile.width, tile.height, options.background, list.limits)?;
    for (target, pixel) in output
        .data_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&page.pixels)
    {
        let color = model.rgb(pixel.straight());
        let alpha = pixel.alpha;
        for (i, out) in target.iter_mut().enumerate().take(3) {
            *out = quantize(color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
        }
        target[3] = quantize(alpha + f64::from(target[3]) / 255.0 * (1.0 - alpha));
    }
    Ok(output)
}

fn mask_value(mask: &SoftMask, pixel: Pixel, model: Model) -> f64 {
    match mask.method {
        MaskMethod::Alpha => pixel.group,
        MaskMethod::Luminosity => {
            let color = if mask.group.attributes.isolated {
                [0, 1, 2, 3].map(|i| pixel.color[i] + (1.0 - pixel.alpha) * mask.backdrop[i])
            } else {
                pixel.color
            };
            model.luminosity(color)
        }
    }
}

fn group_region(group: &Group, tile: Tile, device: Transform) -> Result<Option<Tile>> {
    let Some(bounds) = &group.bounds else {
        return Ok(Some(tile));
    };
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for segment in bounds.segments() {
        let p = match segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => *p,
            Segment::Close => continue,
            Segment::CubicTo(_, _, _) => {
                return Err(Error::invalid_argument("nonlinear group bounds"));
            }
        } * device;
        if !p.x.is_finite() || !p.y.is_finite() {
            return Err(Error::limit("group bounds overflow"));
        }
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    let x0 = coordinate(min.x.floor(), tile.x, tile.x + tile.width);
    let y0 = coordinate(min.y.floor(), tile.y, tile.y + tile.height);
    let x1 = coordinate(max.x.ceil(), tile.x, tile.x + tile.width);
    let y1 = coordinate(max.y.ceil(), tile.y, tile.y + tile.height);
    Ok((x0 < x1 && y0 < y1).then_some(Tile {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }))
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coordinate clamped to validated u32 tile boundaries"
)]
fn coordinate(v: f64, lo: u32, hi: u32) -> u32 {
    v.clamp(f64::from(lo), f64::from(hi)) as u32
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite premultiplied channel clamped and rounded to 0..255"
)]
fn quantize(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5 + 1e-9).floor().min(255.0) as u8
}

fn pattern_source(p: &PathCommand) -> Option<crate::display::ShadingCommand> {
    if let crate::color::Color::Pattern {
        selected: Some(pattern),
        ..
    } = &p.color
    {
        Some(crate::display::ShadingCommand {
            shading: Arc::clone(&pattern.shading),
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
