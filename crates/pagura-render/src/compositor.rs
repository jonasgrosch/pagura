//! Floating-point group composition; quantization occurs at the output boundary.
//!
//! Intermediate channels, alpha and shape use f64 by project policy. Group
//! blending spaces follow 11.7.2. Non-isolated
//! group colours have their initial backdrop removed before final composition
//! (11.4.4). Knockout replaces prior contributions according to shape, not alpha.

mod colorants;
pub(crate) use colorants::Separations;
mod images;
mod opaque_image;
mod patterns;
mod reduced;
mod regions;
mod sampled;
mod transfer;
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
            preserve,
            spot,
        } = source;
        let base = if knockout { initial } else { *self };
        let opacity = if knockout && shape > 0.0 {
            alpha / shape
        } else {
            alpha
        };
        // Compatible overprinting substitutes backdrop components (11.7.4.3).
        // For another blend mode, this is the source of the implicit
        // non-isolated group, before applying the requested outer blend.
        let back = if preserve.iter().any(|&v| v) {
            base.straight()
        } else {
            [0.0; 4]
        };
        let blend_mode = if spot
            && matches!(
                state.blend,
                crate::blend::BlendMode::Difference
                    | crate::blend::BlendMode::Exclusion
                    | crate::blend::BlendMode::Hue
                    | crate::blend::BlendMode::Saturation
                    | crate::blend::BlendMode::Color
                    | crate::blend::BlendMode::Luminosity
            ) {
            crate::blend::BlendMode::Normal
        } else {
            state.blend
        };
        let (color, blend) = if blend_mode == crate::blend::BlendMode::Normal {
            (
                color,
                [0, 1, 2, 3].map(|i| if preserve[i] { back[i] } else { color[i] }),
            )
        } else {
            let color = [0, 1, 2, 3].map(|i| {
                if preserve[i] {
                    (1.0 - base.alpha) * color[i] + base.alpha * back[i]
                } else {
                    color[i]
                }
            });
            (
                color,
                blend_mode.apply(base.straight(), color, model.components()),
            )
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
    preserve: [bool; 4],
    spot: bool,
}
impl Source {
    fn new(color: [f64; 4], alpha: f64, shape: f64) -> Self {
        Self {
            color,
            alpha,
            shape,
            preserve: [false; 4],
            spot: false,
        }
    }
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
    overprint: crate::overprint::Policy,
    elide_opaque_groups: bool,
    separations: Arc<Separations>,
    preview: colorants::Preview,
    spot_batch: Option<usize>,
    mask_colors: bool,
    bounds: regions::Prepared,
    glyphs: crate::display::GlyphCache,
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
    transfers: Option<transfer::Regions>,
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
            .saturating_add(self.glyphs.retained_bytes())
            > self.limits.decoded_bytes
        {
            self.raster.clear();
            self.glyphs.clear();
        }
    }
    /// Checks a group surface over `tile` against the pixel, memory and work
    /// budgets without taking them, returning its pixel count and byte size.
    fn admit_surface(&self, tile: Tile) -> Result<(usize, usize)> {
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .ok()
            .filter(|&v| v <= self.limits.pixels)
            .ok_or(Error::limit("group pixel budget exceeded"))?;
        let bytes = count
            .checked_mul(size_of::<Pixel>())
            .ok_or(Error::limit("group byte size overflow"))?;
        self.allocated
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or(Error::limit("group memory budget exceeded"))?;
        self.work
            .checked_add(count)
            .filter(|&v| v <= self.limits.raster_work)
            .ok_or(Error::limit("compositing work budget exceeded"))?;
        Ok((count, bytes))
    }
    fn surface(&mut self, tile: Tile, initial: Backdrop<'_>) -> Result<Surface> {
        let (count, bytes) = self.admit_surface(tile)?;
        self.trim_raster(bytes);
        self.allocated += bytes;
        self.work += count;
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
            if self.transfers.as_ref().is_some_and(|r| !r.suspended) {
                transfer::validate(command)?;
            }
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
                Command::Glyphs(run) => {
                    self.glyphs(run, target.tile, target, initial, model, knockout, depth)
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
    #[expect(clippy::too_many_arguments, reason = "explicit mark and group state")]
    fn glyphs(
        &mut self,
        run: &crate::display::GlyphRun,
        tile: Tile,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if !run.paint.color.paints() || !path_active(&run.paint) {
            return Ok(());
        }
        self.charge(run.glyphs.len())?;
        self.charge(run.bounds_work()?)?;
        if !regions::run_visible(run, self.device, tile) {
            // Preserve the per-instance bounds charges while avoiding their work.
            let remaining = self.limits.raster_work - self.work;
            if run.glyphs.len() > remaining {
                self.work = self.limits.raster_work;
                return Err(crate::display::contextual(
                    Error::limit("glyph bounds work exceeded"),
                    run.position(remaining),
                ));
            }
            self.work += run.glyphs.len();
            return Ok(());
        }
        let mut clip = None;
        let mut positions = run.positions.iter().peekable();
        let mut position = run.paint.position;
        for (index, &placement) in run.glyphs.iter().enumerate() {
            while let Some(&(_, p)) = positions.next_if(|(i, _)| *i <= index) {
                position = p;
            }
            let result = (|| {
                let hull = run
                    .source(placement)?
                    .device_hull(placement.origin, self.device);
                if !regions::glyph_visible(hull, tile, &mut self.work, self.limits.raster_work)? {
                    return Ok(());
                }
                let geometry = self.glyphs.geometry(
                    run,
                    placement,
                    self.device,
                    &Limits {
                        decoded_bytes: self.limits.decoded_bytes.saturating_sub(self.allocated),
                        ..self.limits
                    },
                    &mut self.raster,
                )?;
                let bytes = geometry.retained_bytes();
                self.trim_raster(bytes);
                self.allocated = self
                    .allocated
                    .checked_add(bytes)
                    .filter(|&n| n <= self.limits.decoded_bytes)
                    .ok_or(Error::limit("glyph geometry memory exceeded"))?;
                let clip = if let Some(clip) = &clip {
                    Arc::clone(clip)
                } else {
                    let value = self.clip(run.paint.clip.as_ref(), tile)?;
                    clip = Some(Arc::clone(&value));
                    value
                };
                let limits = Limits {
                    decoded_bytes: self.limits.decoded_bytes.saturating_sub(self.allocated),
                    ..self.limits
                };
                let Some(mask) = self.raster.clipped_glyph(
                    &clip,
                    geometry,
                    placement.origin * self.device,
                    limits,
                )?
                else {
                    self.allocated -= bytes;
                    return Ok(());
                };
                let Some(mask) = self.pattern_mask(&run.paint, mask)? else {
                    self.allocated -= bytes;
                    return Ok(());
                };
                let result =
                    self.paint_path((&run.paint, &mask), target, initial, model, knockout, depth);
                self.raster.recycle(mask);
                self.allocated -= bytes;
                self.trim_raster(0);
                result
            })();
            result.map_err(|e| crate::display::contextual(e, position))?;
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
        if !path_active(p) {
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
            Some(self.color(&p.color, model, &p.composite, true)?)
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
                && color.preserve == [false; 4]
                && target.fill_solid(start, coverage.len(), color.color)?
            {
                self.transfer_run(p.transfer.as_ref(), &p.composite, (x, y), coverage.len())?;
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
                        alpha: shape,
                        shape,
                        ..color
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
                self.transfer_mark(p.transfer.as_ref(), &p.composite, (x, y), source, true)?;
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
        self.pattern_mask(p, mask)
    }
    fn pattern_mask(&self, p: &PathCommand, mask: Mask) -> Result<Option<Mask>> {
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
            if let Command::Glyphs(run) = command {
                self.glyphs(
                    run,
                    tile,
                    parent,
                    Backdrop::Transparent,
                    parent_model,
                    false,
                    depth + 1,
                )?;
                continue;
            }
            let Command::Path(p) = command else {
                return Err(Error::invalid_argument(
                    "opaque knockout has a non-fill member",
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
        self.transfer_group(group, &mut surface, initial, model, depth)?;
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
                pixel.color = if model == parent_model || self.spot_batch.is_some() {
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
                    spot: self.spot_batch.is_some(),
                    ..Source::new(pixel.color, pixel.group, pixel.shape)
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
                self.transfer_mark(
                    command.transfer.as_ref(),
                    &command.composite,
                    (x, y),
                    source,
                    true,
                )?;
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
                let sample = self.source(
                    &command.shading.space,
                    &values[..command.shading.space.components()],
                    model,
                    &command.composite,
                    false,
                )?;
                for (out, sample) in color.iter_mut().zip(sample.color) {
                    *out += sample;
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
            let suspended = self.transfers.as_ref().is_some_and(|r| r.suspended);
            if let Some(regions) = &mut self.transfers {
                regions.suspended = true;
            }
            let spot_batch = self.spot_batch.take();
            let mask_colors = std::mem::replace(&mut self.mask_colors, true);
            let result = self.commands(
                &mask.group.commands,
                surface,
                initial,
                model,
                mask.group.attributes.knockout,
                depth + 1,
            );
            self.soft_source = previous;
            if let Some(regions) = &mut self.transfers {
                regions.suspended = suspended;
            }
            self.spot_batch = spot_batch;
            self.mask_colors = mask_colors;
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

impl Context {
    /// The compositor state for one render of `list`, and its page blending model.
    fn page(list: &DisplayList, options: &RenderOptions) -> Result<(Self, Model)> {
        let mut crop = PathBuilder::new();
        crop.rect(
            list.crop.x0,
            list.crop.y0,
            list.crop.width(),
            list.crop.height(),
        );
        let mut context = Self {
            overprint: options.overprint.resolve(list.print_intent),
            separations: list.preview_separations(options)?,
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            elide_opaque_groups: !list.group.knockout
                && list.group.model.is_none_or(|m| m == Model::Rgb)
                && normal_scene(&list.commands),
            raster: pagura_raster::Rasterizer::default(),
            bounds: regions::Prepared::new(
                list.device_transform(options),
                list.limits.decoded_bytes,
            ),
            glyphs: crate::display::GlyphCache::new(list.limits.decoded_bytes),
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
            transfers: transfer::Regions::new(&list.commands, list.limits)?,
        };
        context.allocated = context
            .transfers
            .as_ref()
            .map_or(0, transfer::Regions::bytes);
        let model = list.group.model.unwrap_or(if context.separations.enabled {
            Model::Cmyk
        } else {
            Model::Rgb
        });
        context.elide_opaque_groups &= !context.separations.enabled;
        Ok((context, model))
    }
}

pub(crate) fn render(list: &DisplayList, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
    let (mut context, model) = Context::page(list, options)?;
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
            context.render_chunk(list, model, &mut output, tile, region)?;
            x += width;
        }
        y += height;
    }
    Ok(output)
}

impl Context {
    fn render_chunk(
        &mut self,
        list: &DisplayList,
        model: Model,
        output: &mut Pixmap,
        tile: Tile,
        region: Tile,
    ) -> Result<()> {
        if let [Command::Image(image)] = list.commands.as_slice()
            && !list.group.knockout
            && self.copy_opaque_image(image, model, output, tile, region)?
        {
            return Ok(());
        }
        let Tile { x, y, width, .. } = region;
        self.clip = None;
        self.discard_mask();
        self.begin_transfers(region)?;
        let mut page = self.surface(region, Backdrop::Transparent)?;
        self.commands(
            &list.commands,
            &mut page,
            Backdrop::Transparent,
            model,
            list.group.knockout,
            0,
        )?;
        // The page pass has selected every pixel's transfer; spot passes replay
        // the same marks for colourants only.
        if let Some(regions) = &mut self.transfers {
            regions.suspended = true;
        }
        let mut spots = Vec::with_capacity(self.separations.batches());
        for batch in 0..self.separations.batches() {
            self.spot_batch = Some(batch);
            self.clip = None;
            let mut surface = self.surface(region, Backdrop::Transparent)?;
            self.commands(
                &list.commands,
                &mut surface,
                Backdrop::Transparent,
                model,
                list.group.knockout,
                0,
            )?;
            spots.push(surface);
        }
        self.spot_batch = None;
        if let Some(regions) = &mut self.transfers {
            regions.suspended = false;
        }
        let stride =
            usize::try_from(tile.width).map_err(|_| Error::limit("output width exceeds usize"))?;
        for (row, pixels) in page
            .pixels
            .chunks_exact(
                usize::try_from(width).map_err(|_| Error::limit("chunk width exceeds usize"))?,
            )
            .enumerate()
        {
            let start = (usize::try_from(y - tile.y)
                .map_err(|_| Error::limit("output row exceeds usize"))?
                + row)
                * stride
                + usize::try_from(x - tile.x)
                    .map_err(|_| Error::limit("output column exceeds usize"))?;
            for (column, (target, pixel)) in output.data_mut()
                [start * 4..(start + pixels.len()) * 4]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pixels)
                .enumerate()
            {
                let alpha = pixel.alpha;
                if alpha == 0.0 {
                    continue;
                }
                let color = if spots.is_empty() {
                    model.rgb(pixel.straight())
                } else {
                    let index = row
                        * usize::try_from(width)
                            .map_err(|_| Error::limit("chunk width exceeds usize"))?
                        + column;
                    let mut tints = [0.0; 32];
                    for (batch, surface) in spots.iter().enumerate() {
                        for (i, value) in surface.pixels[index].straight().into_iter().enumerate() {
                            tints[batch * 4 + i] = 1.0 - value;
                        }
                    }
                    self.charge(self.separations.len())?;
                    self.preview.rgb(
                        &self.separations,
                        model,
                        pixel.straight(),
                        &tints,
                        &mut self.function_work,
                    )?
                };
                let color = self.transfer_color(row * pixels.len() + column, color)?;
                for (i, out) in target.iter_mut().enumerate().take(3) {
                    *out = quantize(color[i] * alpha + f64::from(*out) / 255.0 * (1.0 - alpha));
                }
                target[3] = quantize(alpha + f64::from(target[3]) / 255.0 * (1.0 - alpha));
            }
        }
        self.allocated -= page.bytes() + spots.iter().map(Surface::bytes).sum::<usize>();
        Ok(())
    }
}

fn opaque_knockout(group: &Group) -> bool {
    group.attributes.view.is_none()
        && !group.attributes.isolated
        && group.attributes.knockout
        && group.attributes.model.is_none_or(|m| m == Model::Rgb)
        && group.composite.simple()
        && group.commands.iter().all(|c| {
            let p = match c {
                Command::Path(p) => p,
                Command::Glyphs(run) => &run.paint,
                _ => return false,
            };
            matches!(p.mark, Mark::Fill(_))
                && p.composite.simple()
                && p.transfer.is_none()
                && matches!(p.color, crate::color::Color::Solid { .. })
        })
}
// A changed backdrop must never feed an amplifying blend, transfer or mask.
fn normal_scene(commands: &[Command]) -> bool {
    commands.iter().all(|command| match command {
        Command::Glyphs(run) => normal_paint(&run.paint),
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

fn normal_paint(p: &PathCommand) -> bool {
    matches!(p.color, crate::color::Color::Solid { .. })
        && p.composite.blend == crate::blend::BlendMode::Normal
        && p.composite.mask.is_none()
        && p.transfer.is_none()
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

fn path_active(p: &PathCommand) -> bool {
    !matches!(
        &p.color,
        crate::color::Color::Pattern { selected: None, .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overprint_list(bytes: &[u8]) -> DisplayList {
        let document = pagura_document::Document::open(
            bytes.to_vec(),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        crate::compile_page(&document, document.page(0).unwrap()).unwrap()
    }

    #[test]
    fn simulation_requires_painted_colorants_and_is_shared_across_tiles() {
        for (bytes, needed) in [
            (
                include_bytes!("../../../corpus/generated/overprint/noop-rgb-multiply-on.pdf")
                    .as_slice(),
                false,
            ),
            (
                include_bytes!("../../../corpus/generated/overprint/fill-zero.pdf"),
                false,
            ),
            (
                include_bytes!(
                    "../../../corpus/generated/overprint/image-ignores-nonzero-mode.pdf"
                ),
                false,
            ),
            (
                include_bytes!(
                    "../../../corpus/generated/overprint/shading-ignores-nonzero-mode.pdf"
                ),
                false,
            ),
            (
                include_bytes!("../../../corpus/generated/overprint/indexed-base-cmyk.pdf"),
                true,
            ),
            (
                include_bytes!("../../../corpus/generated/overprint/separation-on.pdf"),
                true,
            ),
        ] {
            let list = overprint_list(bytes);
            let plan = list.separations().unwrap();
            assert_eq!(plan.enabled, needed);
            let options = RenderOptions::default();
            for x in 0..8 {
                list.render_tile(
                    &options,
                    Tile {
                        x,
                        y: 0,
                        width: 1,
                        height: 8,
                    },
                )
                .unwrap();
                assert!(Arc::ptr_eq(&plan, &list.separations().unwrap()));
            }
        }
    }

    #[test]
    fn icc_rgb_fallback_keeps_opaque_groups_eligible() {
        let list = overprint_list(include_bytes!(
            "../../../corpus/generated/transparency/icc-source-and-group.pdf"
        ));
        assert!(!list.separations().unwrap().enabled);
        let group = list
            .commands
            .iter()
            .find_map(|command| match command {
                Command::Group(group) => Some(group),
                _ => None,
            })
            .unwrap();
        assert_eq!(group.attributes.model, Some(Model::Rgb));
        assert!(normal_scene(&list.commands));
        assert!(group.composite.simple());
    }

    #[test]
    fn separation_batches_and_masks_preserve_tile_parity() {
        for bytes in [
            include_bytes!("../../../corpus/generated/overprint/spot-batches-6.pdf").as_slice(),
            include_bytes!("../../../corpus/generated/overprint/spot-passes-rgb-group.pdf"),
            include_bytes!(
                "../../../corpus/generated/overprint/spot-luminosity-mask-uses-alternate.pdf"
            ),
            include_bytes!("../../../corpus/generated/overprint/fractional-coverage.pdf"),
            include_bytes!("../../../corpus/generated/overprint/combined-fill-stroke-half.pdf"),
        ] {
            let list = overprint_list(bytes);
            for background in [
                pagura_raster::Rgba8::WHITE,
                pagura_raster::Rgba8::TRANSPARENT,
            ] {
                let options = RenderOptions {
                    scale: 1.375,
                    background,
                    ..RenderOptions::default()
                };
                let full = list.render(&options).unwrap();
                let (width, height) = list.device_size(&options).unwrap();
                for y in (0..height).step_by(4) {
                    for x in (0..width).step_by(3) {
                        let tile = Tile {
                            x,
                            y,
                            width: 3.min(width - x),
                            height: 4.min(height - y),
                        };
                        let part = list.render_tile(&options, tile).unwrap();
                        for row in 0..tile.height as usize {
                            let start = ((y as usize + row) * width as usize + x as usize) * 4;
                            let stride = tile.width as usize * 4;
                            assert_eq!(
                                &part.data()[row * stride..(row + 1) * stride],
                                &full.data()[start..start + stride]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spot_surfaces_share_the_process_memory_allowance() {
        let mut list = overprint_list(include_bytes!(
            "../../../corpus/generated/overprint/spot-batches-6.pdf"
        ));
        list.limits.decoded_bytes = 4096;
        assert_eq!(
            list.render(&RenderOptions::default()).unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
    }

    #[test]
    fn sole_rgb_images_preserve_work_and_surface_admission() {
        let mut list = overprint_list(include_bytes!(
            "../../../corpus/generated/paint/image-page-rgb.pdf"
        ));
        let options = RenderOptions::default();
        let expected = list.render(&options).unwrap();
        // One surface visit, one clip visit, one sample per pixel, and one mark.
        list.limits.raster_work = 3 * 32 * 32 + 1;
        assert_eq!(list.render(&options).unwrap(), expected);
        list.limits.raster_work -= 1;
        let error = list.render(&options).unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert!(matches!(
            error.position,
            pagura_core::Position::Stream { .. }
        ));
        for work in [1, 32 * 32] {
            list.limits.raster_work = work;
            let error = list.render(&options).unwrap_err();
            assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
            assert_eq!(error.position, pagura_core::Position::Unknown);
        }
        list.limits.raster_work = Limits::default().raster_work;
        list.limits.decoded_bytes = 32 * 32 * size_of::<Pixel>();
        assert_eq!(list.render(&options).unwrap(), expected);
        list.limits.decoded_bytes -= 1;
        assert_eq!(
            list.render(&options).unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
    }

    #[test]
    fn only_sole_opaque_rgb_images_copy_straight_into_the_output() {
        use crate::display::ImageCommand;
        use crate::interpreter::images::Image;
        fn copies(edit: impl FnOnce(&mut ImageCommand)) -> bool {
            let mut list = overprint_list(include_bytes!(
                "../../../corpus/generated/paint/image-page-rgb.pdf"
            ));
            let [Command::Image(image)] = list.commands.as_mut_slice() else {
                panic!("the fixture paints one image");
            };
            edit(image);
            let [Command::Image(image)] = list.commands.as_slice() else {
                unreachable!();
            };
            let options = RenderOptions::default();
            let (mut context, model) = Context::page(&list, &options).unwrap();
            let (width, height) = list.device_size(&options).unwrap();
            let tile = Tile {
                x: 0,
                y: 0,
                width,
                height,
            };
            let mut output =
                Pixmap::new_with_limits(width, height, options.background, list.limits).unwrap();
            context
                .copy_opaque_image(image, model, &mut output, tile, tile)
                .unwrap()
        }
        fn image(edit: impl FnOnce(&mut Image)) -> impl FnOnce(&mut ImageCommand) {
            |command| edit(Arc::get_mut(&mut command.image).expect("the image is unshared"))
        }
        let transfer = overprint_list(include_bytes!(
            "../../../corpus/generated/recovery/transfer-image.pdf"
        ))
        .commands
        .iter()
        .find_map(|command| match command {
            Command::Image(image) => image.transfer.clone(),
            _ => None,
        })
        .expect("the fixture paints its image through a transfer");
        assert!(copies(|_| {}));
        assert!(!copies(image(|i| i.decode = vec![(1.0, 0.0); 3])));
        assert!(!copies(image(|i| i.decoded.interpolate = true)));
        assert!(!copies(|c| c.composite.alpha = 0.5));
        assert!(!copies(|c| c.transfer = Some(transfer)));
        // A fractional last column, and an image over half the chunk.
        assert!(!copies(|c| c.transform.a = 31.5));
        assert!(!copies(|c| c.transform.a = 16.0));
    }

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
        let source = Source::new(color, 1.0, 1.0);
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
            overprint: crate::overprint::Policy::Ignore,
            elide_opaque_groups: false,
            separations: Arc::new(Separations::default()),
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            raster: pagura_raster::Rasterizer::default(),
            bounds: regions::Prepared::new(Transform::IDENTITY, limits.decoded_bytes),
            glyphs: crate::display::GlyphCache::new(limits.decoded_bytes),
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
            transfers: None,
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
    fn glyph_context(limits: Limits) -> Context {
        let mut crop = PathBuilder::new();
        crop.rect(0.0, 0.0, 4.0, 4.0);
        Context {
            overprint: crate::overprint::Policy::Ignore,
            elide_opaque_groups: false,
            separations: Arc::new(Separations::default()),
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            bounds: regions::Prepared::new(Transform::IDENTITY, limits.decoded_bytes),
            glyphs: crate::display::GlyphCache::new(limits.decoded_bytes),
            raster: pagura_raster::Rasterizer::default(),
            limits,
            device: Transform::IDENTITY,
            page_device: Transform::IDENTITY,
            view_unit: 1.0,
            crop: crop.finish(),
            allocated: 0,
            work: 0,
            clip: None,
            mask: None,
            function_work: 0,
            soft_source: false,
            transfers: None,
        }
    }
    fn glyph_run() -> crate::display::GlyphRun {
        let paint = PathCommand {
            path: Arc::default(),
            mark: Mark::Fill(FillRule::NonZero),
            clip: None,
            color: crate::color::Color::Solid {
                space: crate::color::Space::Rgb,
                components: vec![0.2, 0.5, 0.8],
            },
            position: pagura_core::Position::Unknown,
            transfer: None,
            composite: Composite {
                alpha: 0.5,
                ..Default::default()
            },
        };
        let mut run = crate::display::GlyphRun::new(paint);
        for (width, x) in [(0.4, 0.0), (0.6, 0.2), (0.8, 0.4), (0.4, 100.0)] {
            let mut path = PathBuilder::new();
            path.rect(0.0, 0.0, width, 1.0);
            run.push(
                Arc::new(path.finish()),
                Transform::translate(x, 0.25),
                pagura_core::Position::Unknown,
            )
            .unwrap();
        }
        run
    }
    fn glyph_surface() -> Surface {
        Surface {
            tile: Tile {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            pixels: vec![Pixel::source([0.3, 0.6, 0.9, 0.0], 1.0); 16],
        }
    }
    #[test]
    fn batched_spans_preserve_the_f64_fold_and_cache_blind_limits() {
        let mut run = glyph_run();
        let limits = Limits::default();
        let initial = Backdrop::Color([0.3, 0.6, 0.9, 0.0]);
        for blend in [
            crate::blend::BlendMode::Normal,
            crate::blend::BlendMode::Multiply,
            crate::blend::BlendMode::Dodge,
        ] {
            run.paint.composite.blend = blend;
            for knockout in [false, true] {
                let mut reference = glyph_surface();
                let mut context = glyph_context(limits);
                for &placement in &run.glyphs {
                    let clip = context
                        .clip(run.paint.clip.as_ref(), reference.tile)
                        .unwrap();
                    let geometry = run
                        .geometry(placement, Transform::IDENTITY, &limits)
                        .unwrap();
                    if let Some(mask) = context
                        .raster
                        .clipped_glyph(&clip, geometry, placement.origin, limits)
                        .unwrap()
                    {
                        context
                            .paint_path(
                                (&run.paint, &mask),
                                &mut reference,
                                initial,
                                Model::Rgb,
                                knockout,
                                0,
                            )
                            .unwrap();
                        context.raster.recycle(mask);
                    }
                }
                for capacity in [0, 16 * 1024, limits.decoded_bytes] {
                    let mut context = glyph_context(limits);
                    context.glyphs = crate::display::GlyphCache::new(capacity);
                    let mut actual = glyph_surface();
                    context
                        .glyphs(
                            &run,
                            actual.tile,
                            &mut actual,
                            initial,
                            Model::Rgb,
                            knockout,
                            0,
                        )
                        .unwrap();
                    for (a, b) in actual.pixels.iter().zip(&reference.pixels) {
                        assert_eq!(a.color.map(f64::to_bits), b.color.map(f64::to_bits));
                        assert_eq!(
                            [a.alpha, a.group, a.shape].map(f64::to_bits),
                            [b.alpha, b.group, b.shape].map(f64::to_bits)
                        );
                    }
                }
            }
        }
        for work in [1, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
            let limits = Limits {
                raster_work: work,
                ..limits
            };
            let mut outcomes = Vec::new();
            for capacity in [0, 16 * 1024, limits.decoded_bytes] {
                let mut context = glyph_context(limits);
                context.glyphs = crate::display::GlyphCache::new(capacity);
                let mut target = glyph_surface();
                outcomes.push(
                    context
                        .glyphs(
                            &run,
                            target.tile,
                            &mut target,
                            initial,
                            Model::Rgb,
                            false,
                            0,
                        )
                        .map_err(|e| (e.kind, e.position, e.object)),
                );
            }
            assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
        }
    }
}
