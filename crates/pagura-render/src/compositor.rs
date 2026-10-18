//! Bounded Normal composition with statically selected N16/F32 page storage.
//! Other group buffers retain f64 channels, alpha and shape (ADR 0004). Group
//! blending spaces follow 11.7.2. Non-isolated
//! group colours have their initial backdrop removed before final composition
//! (11.4.4). Knockout replaces prior contributions according to shape, not alpha.

mod colorants;
pub(crate) use colorants::Separations;
mod glyphs;
mod images;
mod narrow;
mod opaque_image;
mod output;
mod page;
mod patterns;
mod plan;
mod reduced;
mod regions;
mod sampled;
mod shading;
mod soft_mask;
mod surface;
mod transfer;
mod view;
pub(crate) use page::render;
pub(crate) use regions::content_bounds;
use regions::group_region;
use shading::pattern_source;
use soft_mask::CachedMask;
use surface::{Backdrop, Frame, Pixel, Source, Surface, runs};

use crate::Tile;
use crate::display::{Clip, Command, Mark, PathCommand};
use crate::transparency::{Group, Model};
use pagura_core::{Error, Limits, Result};
use pagura_raster::{
    FillRule, Mask, Path, PathBuilder, Transform, adjusted_stroke_outline, stroke_outline,
};
use std::sync::Arc;

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
    viewport: Option<Tile>,
}
impl Context {
    /// A context that paints through `device` inside `crop`, without
    /// separations, transfers or opaque-group elision.
    fn new(limits: Limits, device: Transform, crop: Path) -> Self {
        Self {
            overprint: crate::overprint::Policy::Ignore,
            elide_opaque_groups: false,
            separations: Arc::default(),
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            bounds: regions::Prepared::new(device, limits.decoded_bytes),
            glyphs: crate::display::GlyphCache::new(limits.decoded_bytes),
            raster: pagura_raster::Rasterizer::default(),
            limits,
            device,
            page_device: device,
            view_unit: 1.0,
            crop,
            allocated: 0,
            work: 0,
            clip: None,
            mask: None,
            function_work: 0,
            soft_source: false,
            transfers: None,
            viewport: None,
        }
    }
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
        Ok(Surface::new(tile, pixels))
    }
    fn page_surface(
        &mut self,
        tile: Tile,
        format: plan::Format,
        background: [u8; 4],
    ) -> Result<Surface> {
        if format == plan::Format::F64 {
            return self.surface(tile, Backdrop::Transparent);
        }
        let count = tile.width as usize * tile.height as usize;
        let bytes = count * if format == plan::Format::N16 { 8 } else { 16 };
        self.trim_raster(bytes);
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or(Error::limit("colour buffer memory exceeded"))?;
        self.charge(count)?;
        Ok(Surface::with_buffer(
            tile,
            narrow::Buffer::new(format, count, background)?,
        ))
    }
    fn clip(&mut self, clip: Option<&Arc<Clip>>, tile: Tile) -> Result<Arc<Mask>> {
        let tile = self.viewport.unwrap_or(tile);
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
        frame: Frame<'_>,
    ) -> Result<()> {
        if frame.depth >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        // Only knockout elements use the group's initial backdrop (11.4.6).
        // Select this once, so Normal source-over never reads unused pixels.
        let frame = if frame.knockout {
            frame
        } else {
            Frame {
                initial: Backdrop::Transparent,
                ..frame
            }
        };
        self.charge(commands.len())?;
        for command in commands {
            if self.transfers.as_ref().is_some_and(|r| !r.suspended) {
                transfer::validate(command, self.overprint)?;
            }
            if !command.paints() {
                continue;
            }
            let result = match command {
                Command::Glyphs(run) => {
                    self.glyphs(run, self.viewport.unwrap_or(target.tile), target, frame)
                }
                Command::Path(path) => {
                    self.path_in(path, self.viewport.unwrap_or(target.tile), target, frame)
                }
                Command::Image(image) => self.image(image, target, frame),
                Command::Shading(shading) => self.shading(shading, target, frame),
                Command::Group(group) => self.view_group(group, target, frame),
            };
            result.map_err(|e| crate::display::contextual(e, command.position()))?;
        }
        Ok(())
    }
    fn path_in(
        &mut self,
        p: &PathCommand,
        tile: Tile,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        if !path_active(p) {
            return Ok(());
        }
        let Some(mask) = self.path_mask(p, tile)? else {
            return Ok(());
        };
        let result = self.paint_path(p, &mask, target, frame);
        self.raster.recycle(mask);
        result
    }
    // Admission fixes the page and its flattened descendants to RGB.
    fn narrow_path(&mut self, p: &PathCommand, mask: &Mask, target: &mut Surface) -> Result<()> {
        let color = self.color(&p.color, Model::Rgb, &p.composite, true)?.color;
        self.charge(mask.coverage().len())?;
        let tile = target.tile;
        let buffer = target
            .narrow_mut()
            .ok_or(Error::invalid_argument("missing colour buffer"))?;
        let stride = tile.width as usize;
        let (x, y) = mask.origin();
        let offset = (y - tile.y) as usize * stride + (x - tile.x) as usize;
        for (row, column, coverage) in mask.spans() {
            buffer.path(
                offset + row * stride + column,
                coverage,
                color,
                p.composite.alpha,
            )?;
        }
        Ok(())
    }
    fn paint_path(
        &mut self,
        p: &PathCommand,
        mask: &Mask,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        if target.narrow().is_some() {
            return self.narrow_path(p, mask, target);
        }
        if let crate::color::Color::Pattern {
            selected: Some(pattern),
            ..
        } = &p.color
            && let crate::pattern::Kind::Tiling(tiling) = &pattern.kind
        {
            return self.tiling(p, pattern.transform, tiling, mask, target, frame);
        }
        self.paint_color(p, mask, target, frame)
    }
    fn paint_color(
        &mut self,
        p: &PathCommand,
        mask: &Mask,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        let pattern = pattern_source(p);
        self.charge(mask.coverage().len())?;
        let color = if pattern.is_none() {
            Some(self.color(&p.color, frame.model, &p.composite, true)?)
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
        let boundary = self.pattern_boundary(pattern.as_ref(), mask)?;
        let boundary_width = usize::try_from(mask.width())
            .map_err(|_| Error::limit("pattern mask width exceeds usize"))?;
        let soft = p
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, target.tile, frame.model, frame.depth + 1))
            .transpose()?;
        let soft = soft.as_deref().map(Vec::as_slice);
        for run in runs(mask) {
            let run = run?;
            let (x, y, count) = (run.x, run.y, run.coverage.len());
            if run.coverage.first() == Some(&255)
                && p.composite.simple()
                && !frame.knockout
                && let Some(color) = color
                && color.preserve == [false; 4]
                && let Some(start) = target.span(x, y, count)
                && target.fill_solid(start, count, color.color)?
            {
                self.transfer_run(p.transfer.as_ref(), &p.composite, (x, y), count)?;
                continue;
            }
            for (offset, (x, coverage)) in run.pixels().enumerate() {
                let source = if let Some(color) = color {
                    color.covered(coverage)
                } else if let (Some(pattern), Some(inverse)) = (&pattern, inverse) {
                    let rate = if pattern.shading.antialias { 2 } else { 1 };
                    self.charge(rate * rate)?;
                    let Some(source) = self.shading_sample(
                        pattern,
                        inverse,
                        (x, y),
                        frame.model,
                        rate,
                        // The boundary mask spans the coverage mask.
                        boundary.as_ref().map(|mask| {
                            let index = run.row * boundary_width + run.column + offset;
                            mask.coverage()
                                .get(index)
                                .map_or(0.0, |&v| f64::from(v) / 255.0)
                        }),
                    )?
                    else {
                        continue;
                    };
                    source.covered(coverage)
                } else {
                    return Err(Error::invalid_argument("missing pattern source"));
                };
                self.transfer_mark(p.transfer.as_ref(), &p.composite, (x, y), source, true)?;
                target.composite((x, y), source, &p.composite, soft, frame)?;
            }
        }
        Ok(())
    }
    fn path_mask(&mut self, p: &PathCommand, tile: Tile) -> Result<Option<Mask>> {
        // Preserve the explicit singular-pen error even for off-tile marks.
        let inverse = match &p.mark {
            Mark::Fill(_) => Transform::IDENTITY,
            Mark::Stroke { ctm, .. } => ctm.inverse().ok_or(Error::unsupported(
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
            Mark::Stroke {
                pen: stroke,
                ctm,
                adjust,
            } => {
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
            && let Some(path) = &shading.bbox_path
        {
            let Some(clipped) = mask.clipped_path(
                path,
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
    fn narrow_group(
        &mut self,
        group: &Group,
        parent: &mut Surface,
        tile: Tile,
        frame: Frame<'_>,
    ) -> Result<()> {
        let viewport = self.viewport.replace(tile);
        self.clip = None;
        let result = self.commands(
            &group.commands,
            parent,
            Frame::group(Backdrop::Transparent, frame.model, false, frame.depth),
        );
        self.viewport = viewport;
        self.clip = None;
        result
    }
    fn opaque_group(
        &mut self,
        group: &Group,
        parent: &mut Surface,
        tile: Tile,
        frame: Frame<'_>,
    ) -> Result<()> {
        if frame.depth + 1 >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        self.charge(group.commands.len())?;
        let members = Frame::group(Backdrop::Transparent, frame.model, false, frame.depth);
        for command in &group.commands {
            if let Command::Glyphs(run) = command {
                self.glyphs(run, tile, parent, members)?;
                continue;
            }
            let Command::Path(p) = command else {
                return Err(Error::invalid_argument(
                    "opaque knockout has a non-fill member",
                ));
            };
            if command.paints() {
                self.path_in(p, tile, parent, members)
                    .map_err(|e| crate::display::contextual(e, p.position))?;
            }
        }
        Ok(())
    }
    fn group(&mut self, group: &Group, parent: &mut Surface, frame: Frame<'_>) -> Result<()> {
        let Some(tile) = self.region(group, self.viewport.unwrap_or(parent.tile))? else {
            return Ok(());
        };
        if parent.narrow().is_some() {
            return self.narrow_group(group, parent, tile, frame);
        }
        // For opaque Normal members, knockout's shape-weighted replacement
        // reduces to source-over (9.3.8, 11.4.6). Keep the group's mask viewport
        // even though its intermediate colour surface is unnecessary.
        if self.elide_opaque_groups && !frame.knockout && group.attributes.knockout {
            return self.opaque_group(group, parent, tile, frame);
        }
        let model = if group.attributes.isolated {
            group.attributes.model.unwrap_or(frame.model)
        } else {
            frame.model
        };
        let initial = if group.attributes.isolated {
            Backdrop::Transparent
        } else if frame.knockout {
            frame.initial
        } else {
            Backdrop::Surface(parent)
        };
        let mut surface = self.surface(tile, initial)?;
        let members = Frame::group(initial, model, group.attributes.knockout, frame.depth);
        self.transfer_group(group, &mut surface, members)?;
        let soft = group
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, parent.tile, frame.model, frame.depth + 1))
            .transpose()?;
        self.charge(surface.pixels().len())?;
        // Remove the initial backdrop before mutating the parent surface. This
        // also prevents a prior result from becoming a later pixel's backdrop.
        surface.remove_backdrop(initial);
        if model != frame.model && self.spot_batch.is_none() {
            for pixel in surface.pixels_mut().iter_mut().filter(|p| p.shape > 0.0) {
                pixel.color = frame.model.encode_rgb(
                    model.rgb(pixel.color),
                    &group.composite.conversion,
                    &mut self.function_work,
                )?;
            }
        }
        let soft = soft.as_deref().map(Vec::as_slice);
        for (_, point, pixel) in surface.painted() {
            let source = Source {
                spot: self.spot_batch.is_some(),
                ..Source::new(pixel.color, pixel.group, pixel.shape)
            };
            parent.composite(point, source, &group.composite, soft, frame)?;
        }
        self.allocated -= surface.bytes();
        Ok(())
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

fn path_active(p: &PathCommand) -> bool {
    !matches!(
        &p.color,
        crate::color::Color::Pattern { selected: None, .. }
    )
}

#[cfg(test)]
mod tests;
