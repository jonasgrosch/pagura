//! Floating-point group composition; quantization occurs at the output boundary.
//!
//! Intermediate channels, alpha and shape use f64 by project policy. Group
//! blending spaces follow 11.7.2. Non-isolated
//! group colours have their initial backdrop removed before final composition
//! (11.4.4). Knockout replaces prior contributions according to shape, not alpha.

mod images;
mod patterns;
mod reduced;
mod regions;
mod sampled;
mod view;
pub(crate) use regions::content_bounds;
use regions::group_region;

use crate::display::{Clip, Command, DisplayList, Mark, PathCommand};
use crate::transparency::{Composite, Group, MaskMethod, Model, SoftMask};
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Result};
use pagura_raster::{
    FillRule, Mask, Path, PathBuilder, Pixmap, Point, Transform, adjusted_stroke_outline,
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
        // Normal returns the source unchanged (11.3.5); avoid unpremultiplying
        // the backdrop, while retaining the compositing arithmetic and rounding.
        let blend = if state.blend == crate::blend::BlendMode::Normal {
            color
        } else {
            state
                .blend
                .apply(base.straight(), color, model.components())
        };
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
    /// Index of the first of `len` pixels from (x, y), when all lie in one tile row.
    fn span(&self, x: u32, y: u32, len: usize) -> Option<usize> {
        let last = x.checked_add(u32::try_from(len.checked_sub(1)?).ok()?)?;
        self.index(last, y)?;
        self.index(x, y)
    }
    fn fill_solid(&mut self, start: usize, count: usize, color: [f64; 4]) -> Result<bool> {
        let pixels = self
            .pixels
            .get_mut(start..start + count)
            .ok_or(Error::invalid_argument("mask outside group"))?;
        // The Normal formula returns the source exactly at zero or unit backdrop
        // alpha (11.3.3). Other alphas take the general f64 evaluation.
        if !pixels.iter().all(|p| p.alpha == 0.0 || p.alpha == 1.0) {
            return Ok(false);
        }
        pixels.fill(Pixel {
            color,
            alpha: 1.0,
            group: 1.0,
            shape: 1.0,
        });
        Ok(true)
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
    elide_opaque_groups: bool,
    bounds: regions::Prepared,
    raster: pagura_raster::Rasterizer,
    limits: Limits,
    device: Transform,
    page_device: Transform,
    view_unit: f64,
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
    fn region(&mut self, group: &Group, tile: Tile) -> Result<Option<Tile>> {
        group_region(
            group,
            tile,
            self.device,
            &mut self.work,
            self.limits.raster_work,
            &mut self.bounds,
        )
    }
    fn charge(&mut self, work: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(work)
            .filter(|&v| v <= self.limits.raster_work)
            .ok_or(Error::limit("compositing work budget exceeded"))?;
        Ok(())
    }
    fn trim_raster(&mut self, bytes: usize) {
        if self
            .allocated
            .saturating_add(bytes)
            .saturating_add(self.raster.retained_bytes())
            > self.limits.decoded_bytes
        {
            self.raster.clear();
        }
    }
    fn surface(&mut self, tile: Tile, initial: Backdrop<'_>) -> Result<Surface> {
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .ok()
            .filter(|&v| v <= self.limits.pixels)
            .ok_or(Error::limit("group pixel budget exceeded"))?;
        let bytes = count
            .checked_mul(size_of::<Pixel>())
            .ok_or(Error::limit("group byte size overflow"))?;
        self.trim_raster(bytes);
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
        match initial {
            Backdrop::Transparent => pixels.resize(count, Pixel::default()),
            Backdrop::Color(color) => pixels.resize(count, Pixel::source(color, 1.0)),
            Backdrop::Surface(_) => {
                for y in tile.y..tile.y + tile.height {
                    for x in tile.x..tile.x + tile.width {
                        pixels.push(initial.pixel(x, y));
                    }
                }
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
            (&self.crop, self.page_device)
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
        // Only knockout elements use the group's initial backdrop (11.4.6).
        // Select this once, so Normal source-over never reads unused pixels.
        let initial = if knockout {
            initial
        } else {
            Backdrop::Transparent
        };
        self.charge(commands.len())?;
        for command in commands {
            let result = match command {
                Command::Path(path) if !path.color.paints() => Ok(()),
                Command::Shading(s) if !s.shading.space.paints() => Ok(()),
                Command::Image(i)
                    if i.image
                        .space
                        .as_ref()
                        .map_or_else(|| !i.stencil.paints(), |s| !s.paints()) =>
                {
                    Ok(())
                }
                Command::Path(path) => self.path(path, target, initial, model, knockout, depth),
                Command::Image(image) => self.image(image, target, initial, model, knockout, depth),
                Command::Shading(shading) => {
                    self.shading(shading, target, initial, model, knockout, depth)
                }
                Command::Group(group) => {
                    self.view_group(group, target, initial, model, knockout, depth)
                }
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
        self.path_in((p, target.tile), target, initial, model, knockout, depth)
    }
    fn path_in(
        &mut self,
        (p, tile): (&PathCommand, Tile),
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if !path_active(p, model)? {
            return Ok(());
        }
        let Some(mask) = self.path_mask(p, tile)? else {
            return Ok(());
        };
        let result = self.paint_path((p, &mask), target, initial, model, knockout, depth);
        self.raster.recycle(mask);
        result
    }
    fn paint_path(
        &mut self,
        (p, mask): (&PathCommand, &Mask),
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if let crate::color::Color::Pattern {
            selected: Some(pattern),
            ..
        } = &p.color
            && let crate::pattern::Kind::Tiling(tiling) = &pattern.kind
        {
            return self.tiling(
                p,
                pattern.transform,
                tiling,
                mask,
                target,
                initial,
                model,
                knockout,
                depth,
            );
        }
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
        for (row, col, coverage) in mask.spans() {
            let y = oy + u32::try_from(row).map_err(|_| Error::limit("mask row exceeds u32"))?;
            let x = ox + u32::try_from(col).map_err(|_| Error::limit("mask column exceeds u32"))?;
            let start = target
                .span(x, y, coverage.len())
                .ok_or(Error::invalid_argument("mask outside group"))?;
            if coverage[0] == 255
                && p.composite.simple()
                && !knockout
                && let Some(color) = color
                && target.fill_solid(start, coverage.len(), color)?
            {
                continue;
            }
            for (column, &coverage) in coverage.iter().enumerate() {
                let x = x + u32::try_from(column)
                    .map_err(|_| Error::limit("mask column exceeds u32"))?;
                let index = start + column;
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
        // Preserve the explicit singular-pen error even for off-tile marks.
        let inverse = match &p.mark {
            Mark::Fill(_) => Transform::IDENTITY,
            Mark::Stroke(_, ctm, _) => ctm.inverse().ok_or(Error::unsupported(
                crate::features::SINGULAR_STROKE,
                "singular stroke transform",
            ))?,
        };
        if !regions::path_visible(
            p,
            tile,
            self.device,
            &mut self.work,
            self.limits.raster_work,
            &mut self.bounds,
        )? {
            return Ok(None);
        }
        let clip = self.clip(p.clip.as_ref(), tile)?;
        // Retained raster buffers share the live group-surface byte allowance.
        // Surface admissions can release this optional storage before failing.
        let limits = Limits {
            decoded_bytes: self.limits.decoded_bytes.saturating_sub(self.allocated),
            ..self.limits
        };
        let mask = match &p.mark {
            Mark::Fill(rule) => {
                self.raster
                    .clipped_path(&clip, Arc::clone(&p.path), self.device, *rule, limits)?
            }
            Mark::Stroke(stroke, ctm, adjust) => {
                let path = p.path.transformed(inverse);
                let outline = if *adjust {
                    adjusted_stroke_outline
                } else {
                    stroke_outline
                };
                let (path, transform) =
                    outline(&path, ctm.then(self.device), stroke, &self.limits)?;
                self.raster.clipped_path(
                    &clip,
                    Arc::new(path),
                    transform,
                    FillRule::NonZero,
                    limits,
                )?
            }
        };
        let Some(mask) = mask else { return Ok(None) };
        if let crate::color::Color::Pattern {
            selected: Some(pattern),
            ..
        } = &p.color
            && let crate::pattern::Kind::Shading(shading) = &pattern.kind
            && shading.background.is_none()
            && let Some(bbox) = shading.bbox
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
    fn opaque_group(
        &mut self,
        group: &Group,
        parent: &mut Surface,
        parent_model: Model,
        tile: Tile,
        depth: usize,
    ) -> Result<()> {
        if depth + 1 >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        self.charge(group.commands.len())?;
        for command in &group.commands {
            let Command::Path(p) = command else {
                return Err(Error::invalid_argument(
                    "opaque knockout has a non-path member",
                ));
            };
            if p.color.paints() {
                self.path_in(
                    (p, tile),
                    parent,
                    Backdrop::Transparent,
                    parent_model,
                    false,
                    depth + 1,
                )
                .map_err(|e| crate::display::contextual(e, p.position))?;
            }
        }
        Ok(())
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
        let Some(tile) = self.region(group, parent.tile)? else {
            return Ok(());
        };
        // For opaque Normal members, knockout's shape-weighted replacement
        // reduces to source-over (9.3.8, 11.4.6). Keep the group's mask viewport
        // even though its intermediate colour surface is unnecessary.
        // The scene preflight validated every retained knockout group. Masks
        // and patterns disable elision, so no dynamic group bypasses that check.
        if self.elide_opaque_groups && !knockout && group.attributes.knockout {
            return self.opaque_group(group, parent, parent_model, tile, depth);
        }
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
        let origin = clip.origin();
        for (row, column, coverage) in clip.spans() {
            let y = origin.1
                + u32::try_from(row).map_err(|_| Error::limit("shading row exceeds u32"))?;
            for (offset, &coverage) in coverage.iter().enumerate() {
                let column = column + offset;
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
        let mut surface = self
            .region(&mask.group, tile)?
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
        self.trim_raster(bytes);
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
        elide_opaque_groups: !list.group.knockout
            && list.group.model.is_none_or(|m| m == Model::Rgb)
            && normal_scene(&list.commands),
        raster: pagura_raster::Rasterizer::default(),
        bounds: regions::Prepared::new(list.device_transform(options), list.limits.decoded_bytes),
        limits: list.limits,
        device: list.device_transform(options),
        page_device: list.device_transform(options),
        view_unit: list.user_unit,
        crop: crop.finish(),
        allocated: 0,
        work: 0,
        clip: None,
        mask: None,
        function_work: 0,
        soft_source: false,
    };
    let model = list.group.model.unwrap_or(Model::Rgb);
    let mut output =
        Pixmap::new_with_limits(tile.width, tile.height, options.background, list.limits)?;
    // Keep the high-precision page backdrop bounded too. Global device
    // coordinates preserve exact tile parity; one work budget spans all chunks.
    let mut y = tile.y;
    while y < tile.y + tile.height {
        let height = (tile.y + tile.height - y).min(256);
        let mut x = tile.x;
        while x < tile.x + tile.width {
            let width = (tile.x + tile.width - x).min(256);
            let region = Tile {
                x,
                y,
                width,
                height,
            };
            context.clip = None;
            context.discard_mask();
            let mut page = context.surface(region, Backdrop::Transparent)?;
            context.commands(
                &list.commands,
                &mut page,
                Backdrop::Transparent,
                model,
                list.group.knockout,
                0,
            )?;
            let stride = usize::try_from(tile.width)
                .map_err(|_| Error::limit("output width exceeds usize"))?;
            for (row, pixels) in page
                .pixels
                .chunks_exact(
                    usize::try_from(width)
                        .map_err(|_| Error::limit("chunk width exceeds usize"))?,
                )
                .enumerate()
            {
                let start = (usize::try_from(y - tile.y)
                    .map_err(|_| Error::limit("output row exceeds usize"))?
                    + row)
                    * stride
                    + usize::try_from(x - tile.x)
                        .map_err(|_| Error::limit("output column exceeds usize"))?;
                for (target, pixel) in output.data_mut()[start * 4..(start + pixels.len()) * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(pixels)
                {
                    let alpha = pixel.alpha;
                    if alpha == 0.0 {
                        continue;
                    }
                    let color = model.rgb(pixel.straight());
                    for (i, out) in target.iter_mut().enumerate().take(3) {
                        *out = quantize(color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
                    }
                    target[3] = quantize(alpha + f64::from(target[3]) / 255.0 * (1.0 - alpha));
                }
            }
            context.allocated -= page.bytes();
            x += width;
        }
        y += height;
    }
    Ok(output)
}

fn opaque_knockout(group: &Group) -> bool {
    group.attributes.view.is_none()
        && !group.attributes.isolated
        && group.attributes.knockout
        && group.attributes.model.is_none_or(|m| m == Model::Rgb)
        && group.composite.simple()
        && group.commands.iter().all(|c| {
            matches!(c, Command::Path(p)
            if matches!(p.mark, Mark::Fill(_)) && p.composite.simple()
            && p.transfer.is_none() && matches!(p.color, crate::color::Color::Solid { .. }))
        })
}
// A changed backdrop must never feed an amplifying blend, transfer or mask.
fn normal_scene(commands: &[Command]) -> bool {
    commands.iter().all(|command| match command {
        Command::Path(p) => {
            matches!(p.color, crate::color::Color::Solid { .. })
                && p.composite.blend == crate::blend::BlendMode::Normal
                && p.composite.mask.is_none()
                && p.transfer.is_none()
        }
        Command::Image(i) => {
            i.composite.blend == crate::blend::BlendMode::Normal
                && i.composite.mask.is_none()
                && i.transfer.is_none()
        }
        Command::Shading(s) => {
            s.composite.blend == crate::blend::BlendMode::Normal
                && s.composite.mask.is_none()
                && s.transfer.is_none()
        }
        Command::Group(g) => {
            g.composite.simple()
                && g.attributes.model.is_none_or(|m| m == Model::Rgb)
                && (!g.attributes.knockout || opaque_knockout(g))
                && normal_scene(&g.commands)
        }
    })
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

fn path_active(p: &PathCommand, model: Model) -> Result<bool> {
    if matches!(
        &p.color,
        crate::color::Color::Pattern { selected: None, .. }
    ) {
        return Ok(false);
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
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_runs_preserve_unit_and_fractional_backdrop_alpha_evaluation() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let color = [79.0 / 255.0, 0.125, 0.5, 0.0];
        let backdrops = [Pixel::default(), Pixel::source([0.25; 4], 1.0)];
        let mut surface = Surface {
            tile,
            pixels: backdrops.to_vec(),
        };
        assert!(surface.fill_solid(0, 2, color).unwrap());
        let bits = |p: Pixel| {
            (
                p.color.map(f64::to_bits),
                [p.alpha, p.group, p.shape].map(f64::to_bits),
            )
        };
        let source = Source {
            color,
            alpha: 1.0,
            shape: 1.0,
        };
        for (pixel, mut expected) in surface.pixels.iter().zip(backdrops) {
            expected.paint(
                source,
                &Composite::opaque(),
                Pixel::default(),
                false,
                Model::Rgb,
            );
            assert_eq!(bits(*pixel), bits(expected));
        }
        surface.pixels[1].alpha = 0.5;
        let before = surface.pixels[0].color;
        // Fractional backdrops take the general f64 compositing expression.
        assert!(!surface.fill_solid(0, 2, [0.75; 4]).unwrap());
        assert_eq!(
            surface.pixels[0].color.map(f64::to_bits),
            before.map(f64::to_bits)
        );
    }

    #[test]
    fn soft_mask_values_share_the_live_raster_memory_allowance() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        let path = Arc::new(path.finish());
        let clip = Mask::from_path(2, 2, &path, Transform::IDENTITY, FillRule::NonZero, 4).unwrap();
        let mut context = Context {
            elide_opaque_groups: false,
            raster: pagura_raster::Rasterizer::default(),
            bounds: regions::Prepared::new(Transform::IDENTITY, limits.decoded_bytes),
            limits,
            device: Transform::IDENTITY,
            page_device: Transform::IDENTITY,
            view_unit: 1.0,
            crop: (*path).clone(),
            allocated: 0,
            work: 0,
            clip: None,
            mask: None,
            function_work: 0,
            soft_source: false,
        };
        // Rectangles take a closed form without sweep scratch; warm it with a triangle.
        let mut warm = PathBuilder::new();
        warm.move_to(Point::new(0.0, 0.0));
        warm.line_to(Point::new(2.0, 0.0)).unwrap();
        warm.line_to(Point::new(0.0, 2.0)).unwrap();
        warm.close();
        let mask = context
            .raster
            .clipped_path(
                &clip,
                Arc::new(warm.finish()),
                Transform::IDENTITY,
                FillRule::NonZero,
                limits,
            )
            .unwrap()
            .unwrap();
        context.raster.recycle(mask);
        let retained = context.raster.retained_bytes();
        assert!(retained >= 512);
        context.limits.decoded_bytes = retained + 64;
        let mask = Arc::new(SoftMask {
            group: Group {
                position: pagura_core::Position::Unknown,
                bounds: None,
                commands: Vec::new(),
                attributes: crate::transparency::Attributes::default(),
                composite: Composite::opaque(),
            },
            method: MaskMethod::Alpha,
            backdrop: [0.0; 4],
            transfer: None,
        });
        let tile = Tile {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };
        let values = context.soft_mask(&mask, tile, Model::Rgb, 0).unwrap();
        // An empty alpha-mask group contributes no opacity (11.6.5.1).
        assert_eq!(values.as_slice(), [0.0; 64]);
        assert_eq!(context.raster.retained_bytes(), 0);
        assert_eq!(context.allocated, 512);
        assert!(context.allocated <= context.limits.decoded_bytes);
    }
}
