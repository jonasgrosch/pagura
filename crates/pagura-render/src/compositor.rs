//! Bounded Normal composition with statically selected N16/F32 page storage.
//! Other group buffers retain f64 channels, alpha and shape (ADR 0004). Group
//! blending spaces follow 11.7.2. Non-isolated
//! group colours have their initial backdrop removed before final composition
//! (11.4.4). Knockout replaces prior contributions according to shape, not alpha.

mod colorants;
pub(crate) use colorants::Separations;
mod image_interiors;
mod images;
mod lowp;
mod narrow;
mod opaque_image;
mod output;
mod patterns;
mod plan;
mod reduced;
mod regions;
mod sampled;
mod surface;
use surface::Surface;
mod transfer;
mod view;
pub(crate) use regions::content_bounds;
use regions::group_region;

use crate::display::{Clip, Command, DisplayList, Mark, PathCommand};
use crate::transparency::{Composite, Group, MaskMethod, Model, SoftMask};
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Result};
use pagura_raster::{
    ClipRegion, Coverage, FillRule, Mask, Path, PathBuilder, Pixmap, Point, Transform,
    adjusted_stroke_outline, stroke_outline,
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
    fn covered(mut self, coverage: u8) -> Self {
        let factor = f64::from(coverage) / 255.0;
        self.alpha *= factor;
        self.shape *= factor;
        self
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

// Rasterizes one fill against a clip, as `Rasterizer::clipped_path` does.
type Fill<T> = fn(
    &mut pagura_raster::Rasterizer,
    &ClipRegion,
    Arc<Path>,
    Transform,
    FillRule,
    Limits,
) -> Result<Option<T>>;

struct Context {
    overprint: crate::overprint::Policy,
    elide_opaque_groups: bool,
    separations: Arc<Separations>,
    preview: colorants::Preview,
    spot_batch: Option<usize>,
    mask_colors: bool,
    bounds: regions::Prepared,
    glyphs: crate::display::GlyphCache,
    /// Row-fragment glyph masks of the competitive policy.
    fragments: Option<Fragments>,
    /// Storage lent for the next byte page surface.
    spare: Vec<u8>,
    /// The last expanded indexed image of byte-page interiors.
    palette: Option<image_interiors::Palette>,
    raster: pagura_raster::Rasterizer,
    limits: Limits,
    device: Transform,
    page_device: Transform,
    view_unit: f64,
    crop: Path,
    allocated: usize,
    work: usize,
    clip: Option<(usize, Tile, Arc<ClipRegion>)>,
    mask: Option<CachedMask>,
    function_work: usize,
    soft_source: bool,
    transfers: Option<transfer::Regions>,
    viewport: Option<Tile>,
}
struct CachedMask {
    id: usize,
    tile: Tile,
    model: Model,
    values: Arc<Vec<f64>>,
}
impl Context {
    fn new(
        list: &DisplayList,
        options: &RenderOptions,
        plan: &plan::Plan,
        separations: Arc<Separations>,
    ) -> Result<Self> {
        let mut crop = PathBuilder::new();
        crop.rect(
            list.crop.x0,
            list.crop.y0,
            list.crop.width(),
            list.crop.height(),
        );
        let transfers = transfer::Regions::new(&list.commands, list.limits)?;
        let allocated = transfers.as_ref().map_or(0, transfer::Regions::bytes);
        let elide_opaque_groups = plan.format == plan::Format::F64
            && !separations.enabled
            && !list.group.knockout
            && list.group.model.is_none_or(|m| m == Model::Rgb)
            && normal_scene(&list.commands);
        Ok(Self {
            overprint: options.overprint.resolve(list.print_intent),
            separations,
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            elide_opaque_groups,
            raster: pagura_raster::Rasterizer::default(),
            bounds: regions::Prepared::new(
                list.device_transform(options),
                list.limits.decoded_bytes,
            ),
            fragments: None,
            spare: Vec::new(),
            palette: None,
            glyphs: crate::display::GlyphCache::new(list.limits.decoded_bytes),
            limits: list.limits,
            device: list.device_transform(options),
            page_device: list.device_transform(options),
            view_unit: list.user_unit,
            crop: crop.finish(),
            allocated,
            work: plan.work,
            clip: None,
            mask: None,
            function_work: 0,
            soft_source: false,
            transfers,
            viewport: None,
        })
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
        if self.allocated.saturating_add(bytes) > self.limits.decoded_bytes {
            self.clear_palette();
        }
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
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .ok()
            .filter(|&v| v <= self.limits.pixels)
            .ok_or(Error::limit("page pixel budget exceeded"))?;
        let bytes = match format {
            plan::Format::U8 => narrow::Bytes::charge(count, tile.height as usize),
            plan::Format::N16 => count.checked_mul(8),
            _ => count.checked_mul(16),
        }
        .ok_or(Error::limit("colour buffer byte size overflow"))?;
        self.trim_raster(bytes);
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or(Error::limit("colour buffer memory exceeded"))?;
        self.charge(count)?;
        let buffer = if format == plan::Format::U8 {
            narrow::Buffer::U8(narrow::Bytes::new(
                (tile.width as usize, tile.height as usize),
                background,
                std::mem::take(&mut self.spare),
            )?)
        } else {
            narrow::Buffer::new(format, count, background)?
        };
        Ok(Surface::with_buffer(tile, buffer))
    }
    // Clipping paths stay geometry: each mark integrates them with its own
    // outline, so a clip costs no coverage of its own (8.5.4).
    fn clip(&mut self, clip: Option<&Arc<Clip>>, tile: Tile) -> Result<Arc<ClipRegion>> {
        let tile = self.viewport.unwrap_or(tile);
        let id = clip.map_or(0, |c| Arc::as_ptr(c) as usize);
        if let Some((key, cached, region)) = &self.clip
            && *key == id
            && *cached == tile
        {
            return Ok(Arc::clone(region));
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
        let mut region = ClipRegion::from_path(
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
            let next = ClipRegion::from_path(
                tile.width,
                tile.height,
                &node.path,
                self.device,
                node.rule,
                self.limits,
                (tile.x, tile.y),
            )?;
            region.intersect(&next)?;
        }
        // Under the clip-product policy the region's own coverage is
        // integrated once here and multiplied into each mark.
        self.raster.prepare_clip(&mut region)?;
        let region = Arc::new(region);
        self.clip = Some((id, tile, Arc::clone(&region)));
        Ok(region)
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
                Command::Glyphs(run) => self.glyphs(
                    run,
                    self.viewport.unwrap_or(target.tile),
                    target,
                    initial,
                    model,
                    knockout,
                    depth,
                ),
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
        if let Some(mut fragments) = self.fragments.take() {
            let result = if fragment_paint(&run.paint)
                && matches!(target.narrow(), Some(narrow::Buffer::U8(_)))
            {
                Some(self.fragment_glyphs(run, tile, target, &mut fragments))
            } else {
                None
            };
            self.fragments = Some(fragments);
            if let Some(result) = result {
                return result;
            }
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
    /// Paints a run of solid glyphs into byte storage from row-fragment
    /// masks, scaled by the clip's prepared product. Work is charged for each
    /// glyph's bounds and every level of its mask, as for a dense mask.
    fn fragment_glyphs(
        &mut self,
        run: &crate::display::GlyphRun,
        tile: Tile,
        target: &mut Surface,
        fragments: &mut Fragments,
    ) -> Result<()> {
        let color = self
            .color(&run.paint.color, Model::Rgb, &run.paint.composite, true)?
            .color;
        let (color, alpha) = narrow::byte_source(color, run.paint.composite.alpha);
        let region = self.clip(run.paint.clip.as_ref(), tile)?;
        let viewport = region_rectangle(region.origin(), (region.width(), region.height()));
        let clip = narrow::MaskClip {
            product: region.product().map(|product| (product, viewport)),
            rectangle: region.rectangle(),
        };
        if clip.product.is_none() && clip.rectangle.is_none() && !region.unrestricted() {
            return Err(Error::invalid_argument("clip has no prepared product"));
        }
        let surface = target.tile;
        let bounds = region_rectangle((surface.x, surface.y), (surface.width, surface.height));
        let bounds = [
            bounds[0].max(viewport[0]),
            bounds[1].max(viewport[1]),
            bounds[2].min(viewport[2]),
            bounds[3].min(viewport[3]),
        ];
        let Some(narrow::Buffer::U8(bytes)) = target.narrow_mut() else {
            return Err(Error::invalid_argument("missing byte buffer"));
        };
        let mut positions = run.positions.iter().peekable();
        let mut position = run.paint.position;
        // Each source's device hull relative to its origin, computed once.
        let mut hulls = vec![None; run.sources.len()];
        let (left, top) = (f64::from(tile.x) - 1.0, f64::from(tile.y) - 1.0);
        let (right, bottom) = (
            f64::from(tile.x + tile.width) + 1.0,
            f64::from(tile.y + tile.height) + 1.0,
        );
        for (index, &placement) in run.glyphs.iter().enumerate() {
            while let Some(&(_, p)) = positions.next_if(|(i, _)| *i <= index) {
                position = p;
            }
            let result = (|| {
                let source = run.source(placement)?;
                let relative = *hulls
                    .get_mut(placement.source as usize)
                    .ok_or(Error::invalid_argument("invalid glyph source"))?
                    .get_or_insert_with(|| source.relative_hull(self.device));
                // Masks are clipped to the surface exactly; the hull only
                // skips glyphs that lie wholly outside it.
                let origin = placement.origin * self.device;
                self.charge(1)?;
                if origin.x.is_nan() || origin.y.is_nan() {
                    return Err(Error::limit("glyph bounds overflow"));
                }
                if !(relative[1].x + origin.x >= left
                    && relative[0].x + origin.x < right
                    && relative[1].y + origin.y >= top
                    && relative[0].y + origin.y < bottom)
                {
                    return Ok(());
                }
                let transform = Transform {
                    e: placement.origin.x,
                    f: placement.origin.y,
                    ..source.linear
                }
                .then(self.device);
                let (mask, (dx, dy)) = fragments.cache.coverage(
                    &source.outline,
                    (transform, fragments.resolution),
                    self.limits,
                    &mut fragments.accumulator,
                )?;
                self.work = self
                    .work
                    .checked_add(mask.data.len())
                    .filter(|&v| v <= self.limits.raster_work)
                    .ok_or(Error::limit("compositing work budget exceeded"))?;
                let origin = (
                    i64::from(mask.x) + i64::from(dx),
                    i64::from(mask.y) + i64::from(dy),
                );
                bytes.mask(
                    (mask.data, mask.width, origin),
                    ((i64::from(surface.x), i64::from(surface.y)), bounds),
                    (color, alpha),
                    clip,
                    &mut fragments.row,
                )
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
        self.path_in(
            (p, self.viewport.unwrap_or(target.tile)),
            target,
            initial,
            model,
            knockout,
            depth,
        )
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
        // Narrow storage paints only solid colours, which need no pattern
        // bounds, so rectangles keep their closed-form runs.
        if target.narrow().is_some() && matches!(p.color, crate::color::Color::Solid { .. }) {
            let Some(coverage) =
                self.path_fill(p, tile, pagura_raster::Rasterizer::clipped_coverage)?
            else {
                return Ok(());
            };
            let result = match &coverage {
                Coverage::Mask(mask) => self.narrow_path(
                    p,
                    (mask.origin(), mask.coverage().len()),
                    mask.spans(),
                    Some((mask.width() as usize, mask.coverage())),
                    target,
                ),
                Coverage::Rectangle(rectangle) => self.narrow_path(
                    p,
                    (rectangle.origin(), rectangle.area()),
                    rectangle.spans(),
                    None,
                    target,
                ),
            };
            self.raster.recycle(coverage);
            return result;
        }
        let Some(mask) = self.path_mask(p, tile)? else {
            return Ok(());
        };
        let result = self.paint_path((p, &mask), target, initial, model, knockout, depth);
        self.raster.recycle(mask);
        result
    }
    // Admission fixes the page and its flattened descendants to RGB. Work is
    // charged for every sample of the coverage region, as for a dense mask.
    fn narrow_path<'a>(
        &mut self,
        p: &PathCommand,
        (origin, area): ((u32, u32), usize),
        spans: impl Iterator<Item = (usize, usize, &'a [u8])>,
        dense: Option<(usize, &[u8])>,
        target: &mut Surface,
    ) -> Result<()> {
        let color = self.color(&p.color, Model::Rgb, &p.composite, true)?.color;
        self.charge(area)?;
        let tile = target.tile;
        let buffer = target
            .narrow_mut()
            .ok_or(Error::invalid_argument("missing colour buffer"))?;
        let stride = tile.width as usize;
        let (x, y) = origin;
        let offset = (y - tile.y) as usize * stride + (x - tile.x) as usize;
        if let Some((width, coverage)) = dense
            && buffer.small_mask((offset, stride, width), coverage, color, p.composite.alpha)?
        {
            return Ok(());
        }
        for (row, column, coverage) in spans {
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
        (p, mask): (&PathCommand, &Mask),
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if target.narrow().is_some() && matches!(p.color, crate::color::Color::Solid { .. }) {
            let area = mask.coverage().len();
            let dense = (mask.width() as usize, mask.coverage());
            return self.narrow_path(p, (mask.origin(), area), mask.spans(), Some(dense), target);
        }
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
        self.paint_color((p, mask), target, initial, model, knockout, depth)
    }
    fn paint_color(
        &mut self,
        (p, mask): (&PathCommand, &Mask),
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
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
        let boundary = self.pattern_boundary(pattern.as_ref(), mask)?;
        let boundary_width = usize::try_from(mask.width())
            .map_err(|_| Error::limit("pattern mask width exceeds usize"))?;
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
                let source = if let Some(color) = color {
                    color.covered(coverage)
                } else if let (Some(pattern), Some(inverse)) = (&pattern, inverse) {
                    let rate = if pattern.shading.antialias { 2 } else { 1 };
                    self.charge(rate * rate)?;
                    let Some(source) = self.shading_sample(
                        pattern,
                        inverse,
                        (x, y),
                        model,
                        rate,
                        boundary.as_ref().map(|mask| {
                            f64::from(mask.coverage()[row * boundary_width + col + column]) / 255.0
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
                let source = source.apply(&p.composite, mask_value);
                if let Some(buffer) = target.narrow_mut() {
                    buffer.paint(index, source)?;
                    continue;
                }
                let pixel = target
                    .pixels_mut()
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("invalid group index"))?;
                pixel.paint(source, &p.composite, initial.pixel(x, y), knockout, model);
            }
        }
        Ok(())
    }
    fn path_mask(&mut self, p: &PathCommand, tile: Tile) -> Result<Option<Mask>> {
        let Some(mask) = self.path_fill(p, tile, pagura_raster::Rasterizer::clipped_path)? else {
            return Ok(None);
        };
        self.pattern_mask(p, mask)
    }
    // Rasterizes a fill or stroke outline with `fill` against the mark's clip.
    fn path_fill<T>(&mut self, p: &PathCommand, tile: Tile, fill: Fill<T>) -> Result<Option<T>> {
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
        match &p.mark {
            Mark::Fill(rule) => fill(
                &mut self.raster,
                &clip,
                Arc::clone(&p.path),
                self.device,
                *rule,
                limits,
            ),
            Mark::Stroke(stroke, ctm, adjust) => {
                let path = p.path.transformed(inverse);
                let outline = if *adjust {
                    adjusted_stroke_outline
                } else {
                    stroke_outline
                };
                let (path, transform) =
                    outline(&path, ctm.then(self.device), stroke, &self.limits)?;
                fill(
                    &mut self.raster,
                    &clip,
                    Arc::new(path),
                    transform,
                    FillRule::NonZero,
                    limits,
                )
            }
        }
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
        model: Model,
        tile: Tile,
        depth: usize,
    ) -> Result<()> {
        let viewport = self.viewport.replace(tile);
        self.clip = None;
        let result = self.commands(
            &group.commands,
            parent,
            Backdrop::Transparent,
            model,
            false,
            depth + 1,
        );
        self.viewport = viewport;
        self.clip = None;
        result
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
        let Some(tile) = self.region(group, self.viewport.unwrap_or(parent.tile))? else {
            return Ok(());
        };
        if parent.narrow().is_some() {
            return self.narrow_group(group, parent, parent_model, tile, depth);
        }
        // For opaque Normal members, knockout's shape-weighted replacement
        // reduces to source-over (9.3.8, 11.4.6). Keep the group's mask viewport
        // even though its intermediate colour surface is unnecessary.
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
        self.charge(surface.pixels().len())?;
        let width =
            usize::try_from(tile.width).map_err(|_| Error::limit("group width exceeds usize"))?;
        // Remove the initial backdrop before mutating the parent surface. This
        // also prevents a prior result from becoming a later pixel's backdrop.
        for (row, pixels) in surface.pixels_mut().chunks_exact_mut(width).enumerate() {
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
        for (row, pixels) in surface.pixels().chunks_exact(width).enumerate() {
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
                    .pixels_mut()
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
    // Keep sampling and painting out of the command dispatch.
    #[inline(never)]
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
        let clip = if let Some(path) = &command.shading.bbox_path {
            let Some(mask) = clip.clipped_path(path, transform, FillRule::NonZero, self.limits)?
            else {
                return Ok(());
            };
            mask
        } else {
            clip.mask()?
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
                let source = self.shading_sample(command, inverse, (x, y), model, rate, None)?;
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
                if let Some(buffer) = target.narrow_mut() {
                    buffer.paint(index, source.apply(&command.composite, mask))?;
                    continue;
                }
                target
                    .pixels_mut()
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
        boundary: Option<f64>,
    ) -> Result<Option<Source>> {
        let background = boundary
            .and(command.shading.background)
            .map(|values| {
                self.source(
                    &command.shading.space,
                    &values[..command.shading.space.components()],
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
                let inside = certified
                    || command.shading.bbox.is_none_or(|b| {
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
                    .and_then(|s| s.index(x, y).and_then(|i| s.pixels().get(i)))
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

/// The band schedule and approximations of [`crate::Accuracy::Competitive`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Policy {
    /// Rows per full-width band of narrow storage, between 1 and 16384.
    /// Retained groups keep square chunks of at most `plan.side` rows.
    pub(crate) rows: u32,
    /// Signed-cell polygons at this device tolerance; `None` keeps exact coverage.
    pub(crate) curve_tolerance: Option<f64>,
    /// One-round byte Normal composition in place of admitted N16/F32 storage.
    pub(crate) byte_composition: bool,
    /// Multiply independently sampled clip coverages (requires signed cells).
    pub(crate) clip_product: bool,
    /// Keep geometric integration of compound paths without a stroke boundary.
    pub(crate) geometric_compounds: bool,
}

impl Policy {
    /// [`crate::Accuracy::Competitive`].
    pub(crate) const COMPETITIVE: Self = Self {
        rows: 256,
        curve_tolerance: Some(1.0 / 64.0),
        byte_composition: true,
        clip_product: true,
        geometric_compounds: true,
    };

    fn rasterizer(self) -> Result<pagura_raster::Rasterizer> {
        let raster = match (self.curve_tolerance, self.clip_product) {
            (Some(tolerance), true) => {
                pagura_raster::Rasterizer::with_approximate_clip_product(tolerance)
            }
            (Some(tolerance), false) => {
                pagura_raster::Rasterizer::with_approximate_cells(tolerance)
            }
            (None, false) => Ok(pagura_raster::Rasterizer::default()),
            (None, true) => Err(Error::invalid_argument(
                "clip product requires approximate cells",
            )),
        }?;
        Ok(if self.geometric_compounds {
            raster.with_geometric_compounds()
        } else {
            raster
        })
    }
}

/// Storage a rendering context lends to competitive renders: its retained
/// glyph masks and a surface's storage returned by `RenderContext::recycle`.
/// A contended or absent lender only costs time: masks are a pure function
/// of their key, and every surface is filled before use.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Lent<'a> {
    pub(crate) glyphs: Option<&'a std::sync::Mutex<crate::fragments::GlyphCache>>,
    pub(crate) spare: Option<&'a std::sync::Mutex<Vec<u8>>>,
}

/// Row-fragment glyph masks and their working storage for one render.
struct Fragments {
    cache: crate::fragments::GlyphCache,
    accumulator: crate::fragments::Accumulator,
    resolution: crate::fragments::Resolution,
    /// One row of a mask scaled by its clip.
    row: Vec<u8>,
}

pub(crate) fn render(list: &DisplayList, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
    render_policy(list, options, tile, None)
}

/// Renders `tile` under [`Policy::COMPETITIVE`], with storage from `lent`.
pub(crate) fn render_competitive(
    list: &DisplayList,
    options: &RenderOptions,
    tile: Tile,
    lent: Lent<'_>,
) -> Result<Pixmap> {
    let mut shared = lent.glyphs.and_then(|cache| cache.try_lock().ok());
    let cache = shared.as_deref_mut().map_or_else(
        || crate::fragments::GlyphCache::new(crate::fragments::CACHE_BYTES),
        std::mem::take,
    );
    let spare = lent
        .spare
        .and_then(|spare| spare.try_lock().ok())
        .map(|mut spare| std::mem::take(&mut *spare))
        .unwrap_or_default();
    let mut fragments = Some(Fragments {
        cache,
        accumulator: crate::fragments::Accumulator::default(),
        resolution: crate::fragments::Resolution::new(options),
        row: Vec::new(),
    });
    let result = render_policy(
        list,
        options,
        tile,
        Some((Policy::COMPETITIVE, &mut fragments, spare)),
    );
    if let (Some(shared), Some(fragments)) = (shared.as_deref_mut(), fragments) {
        *shared = fragments.cache;
    }
    result
}

// Keep the large general compositor out of the opaque page dispatch.
#[inline(never)]
fn render_policy(
    list: &DisplayList,
    options: &RenderOptions,
    tile: Tile,
    competitive: Option<(Policy, &mut Option<Fragments>, Vec<u8>)>,
) -> Result<Pixmap> {
    let (bands, fragments, spare) = match competitive {
        Some((policy, fragments, spare)) => (Some(policy), Some(fragments), spare),
        None => (None, None, Vec::new()),
    };
    if let Some(bands) = bands
        && !(1..=16_384).contains(&bands.rows)
    {
        return Err(Error::invalid_argument("band height outside 1..=16384"));
    }
    let separations = list.preview_separations(options)?;
    let mut plan = if bands.is_some_and(|b| b.byte_composition) {
        plan::byte_page(list, &separations)?
    } else {
        plan::page(list, &separations)?
    };
    if bands.is_some_and(|b| b.byte_composition) && plan.format != plan::Format::F64 {
        plan.format = plan::Format::U8;
    }
    let mut context = Context::new(list, options, &plan, separations)?;
    if let Some(bands) = bands {
        context.raster = bands.rasterizer()?;
        context.spare = spare;
    }
    // Solid glyphs in byte storage take masks from row fragments when given.
    let mut fragments = fragments.filter(|_| bands.is_some());
    if let Some(slot) = fragments.as_deref_mut() {
        context.fragments = slot.take();
    }
    let result = context.render_bands(list, options, tile, (plan, bands));
    if let Some(slot) = fragments {
        *slot = context.fragments.take();
    }
    result
}

impl Context {
    fn render_bands(
        &mut self,
        list: &DisplayList,
        options: &RenderOptions,
        tile: Tile,
        (plan, bands): (plan::Plan, Option<Policy>),
    ) -> Result<Pixmap> {
        let context = self;
        let model = list.group.model.unwrap_or(if context.separations.enabled {
            Model::Cmyk
        } else {
            Model::Rgb
        });
        // Wide retained groups have a much larger live working set. Keep their
        // existing square chunks; narrow pages can retain full-width byte bands.
        let band_width = if bands.is_some() && plan.format != plan::Format::F64 {
            tile.width
        } else {
            plan.side
        };
        let band_height = bands.map_or(plan.side, |b| {
            if plan.format == plan::Format::F64 {
                b.rows.min(plan.side)
            } else {
                b.rows
            }
        });
        if plan.format == plan::Format::U8
            && band_width >= tile.width
            && options.background.a == 255
            && !matches!(list.commands.as_slice(), [Command::Image(_)])
        {
            return context.byte_page(list, model, tile, band_height, options.background);
        }
        let mut output = Pixmap::with_storage(
            tile.width,
            tile.height,
            options.background,
            list.limits,
            std::mem::take(&mut context.spare),
        )?;
        let mut y = tile.y;
        while y < tile.y + tile.height {
            let height = (tile.y + tile.height - y).min(band_height);
            let mut x = tile.x;
            while x < tile.x + tile.width {
                let width = (tile.x + tile.width - x).min(band_width);
                let region = Tile {
                    x,
                    y,
                    width,
                    height,
                };
                context.render_chunk(
                    list,
                    model,
                    &mut output,
                    tile,
                    region,
                    (
                        plan.format,
                        [
                            options.background.r,
                            options.background.g,
                            options.background.b,
                            options.background.a,
                        ],
                    ),
                )?;
                x += width;
            }
            y += height;
        }
        Ok(output)
    }
}

impl Context {
    /// Renders an opaque byte page in full-width bands of `rows` rows. The
    /// page's storage is filled once; each band's marks are painted in place
    /// through a window of its rows, so bands need no storage or copy of
    /// their own.
    fn byte_page(
        &mut self,
        list: &DisplayList,
        model: Model,
        tile: Tile,
        rows: u32,
        background: pagura_raster::Rgba8,
    ) -> Result<Pixmap> {
        self.begin_transfers(tile)?;
        let background = [background.r, background.g, background.b, 255];
        let mut page = self.page_surface(tile, plan::Format::U8, background)?;
        let mut y = tile.y;
        while y < tile.y + tile.height {
            let height = (tile.y + tile.height - y).min(rows);
            let band = Tile {
                x: tile.x,
                y,
                width: tile.width,
                height,
            };
            page.window(band)?;
            self.clip = None;
            self.discard_mask();
            self.commands(
                &list.commands,
                &mut page,
                Backdrop::Transparent,
                model,
                list.group.knockout,
                0,
            )?;
            y += height;
        }
        Pixmap::from_premultiplied(tile.width, tile.height, page.into_rgba()?, list.limits)
    }
    fn render_chunk(
        &mut self,
        list: &DisplayList,
        model: Model,
        output: &mut Pixmap,
        tile: Tile,
        region: Tile,
        (format, background): (plan::Format, [u8; 4]),
    ) -> Result<()> {
        if let [Command::Image(image)] = list.commands.as_slice()
            && !list.group.knockout
            && self.paint_opaque_image(image, model, output, (tile, region), background)?
        {
            return Ok(());
        }
        self.clip = None;
        self.discard_mask();
        self.begin_transfers(region)?;
        let mut page = self.page_surface(region, format, background)?;
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
        self.write_output(&page, &spots, model, output, tile)?;
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

/// Whether a glyph run in byte storage takes row-fragment masks: a solid
/// colour, as byte storage paints any other mark through its coverage.
fn fragment_paint(p: &PathCommand) -> bool {
    matches!(p.color, crate::color::Color::Solid { .. })
}

/// The device rectangle `[x0, y0, x1, y1]` of a viewport.
fn region_rectangle(origin: (u32, u32), (width, height): (u32, u32)) -> [i64; 4] {
    let (x, y) = (i64::from(origin.0), i64::from(origin.1));
    [x, y, x + i64::from(width), y + i64::from(height)]
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
    fn full_normal_stores_preserve_the_f64_evaluation() {
        let state = Composite::opaque();
        for alpha in [0.0, 1.0] {
            for backdrop in [-0.0, 0.0, 0.1, 0.5, 1.0] {
                for source in [-0.0, 0.0, f64::MIN_POSITIVE, 0.3, 0.5, 1.0] {
                    let mut pixel = Pixel {
                        color: [backdrop; 4],
                        alpha,
                        group: 0.5,
                        shape: 0.25,
                    };
                    let opacity = 1.0;
                    let expected = (1.0 - opacity) * backdrop
                        + opacity * ((1.0 - alpha) * source + alpha * source);
                    pixel.paint(
                        Source::new([source; 4], 1.0, 1.0),
                        &state,
                        Pixel::default(),
                        false,
                        Model::Rgb,
                    );
                    assert_eq!(pixel.color.map(f64::to_bits), [expected.to_bits(); 4]);
                    assert_eq!([pixel.alpha, pixel.group, pixel.shape], [1.0; 3]);
                }
            }
        }
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
        assert_eq!(list.render(&options).unwrap(), expected);
        list.limits.decoded_bytes = 1000;
        assert_eq!(
            list.render(&options).unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
    }

    #[test]
    fn only_sole_opaque_images_paint_straight_into_the_output() {
        use crate::display::ImageCommand;
        use crate::interpreter::images::Image;
        fn paints(background: [u8; 4], edit: impl FnOnce(&mut ImageCommand)) -> bool {
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
            let separations = list.preview_separations(&options).unwrap();
            let plan = plan::page(&list, &separations).unwrap();
            let mut context = Context::new(&list, &options, &plan, separations).unwrap();
            let model = Model::Rgb;
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
                .paint_opaque_image(image, model, &mut output, (tile, tile), background)
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
        let opaque = [255; 4];
        let clear = [255, 255, 255, 0];
        assert!(paints(opaque, |_| {}));
        // Inverted Decode keeps integer levels; interpolation is bilinear.
        assert!(paints(opaque, image(|i| i.decode = vec![(1.0, 0.0); 3])));
        assert!(paints(opaque, image(|i| i.decoded.interpolate = true)));
        // Minification takes exact area averages.
        assert!(paints(opaque, |c| c.transform.a = 1.5));
        // A fractional last column, and an image over half the chunk, mix
        // their partial coverage with the opaque background.
        assert!(paints(opaque, |c| c.transform.a = 31.5));
        assert!(paints(opaque, |c| c.transform.a = 16.0));
        // A translucent background keeps the group path.
        assert!(!paints(clear, |_| {}));
        assert!(!paints(clear, |c| c.transform.a = 16.0));
        // Fractional Decode levels, translucency, transfers and rotations
        // that are not quarter turns keep the group path.
        assert!(!paints(opaque, image(|i| i.decode = vec![(0.25, 0.75); 3])));
        assert!(!paints(opaque, |c| c.composite.alpha = 0.5));
        assert!(!paints(opaque, |c| c.transfer = Some(transfer)));
        assert!(!paints(opaque, |c| {
            c.transform = Transform::new(1.0, 1.0, -1.0, 1.0, 16.0, 0.0);
        }));
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
        let mut surface = Surface::new(tile, backdrops.to_vec());
        assert!(surface.fill_solid(0, 2, color).unwrap());
        let bits = |p: Pixel| {
            (
                p.color.map(f64::to_bits),
                [p.alpha, p.group, p.shape].map(f64::to_bits),
            )
        };
        let source = Source::new(color, 1.0, 1.0);
        for (pixel, mut expected) in surface.pixels().iter().zip(backdrops) {
            expected.paint(
                source,
                &Composite::opaque(),
                Pixel::default(),
                false,
                Model::Rgb,
            );
            assert_eq!(bits(*pixel), bits(expected));
        }
        surface.pixels_mut()[1].alpha = 0.5;
        let before = surface.pixels()[0].color;
        // Fractional backdrops take the general f64 compositing expression.
        assert!(!surface.fill_solid(0, 2, [0.75; 4]).unwrap());
        assert_eq!(
            surface.pixels()[0].color.map(f64::to_bits),
            before.map(f64::to_bits)
        );
    }

    #[test]
    fn soft_mask_values_share_the_live_raster_memory_allowance() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        let path = Arc::new(path.finish());
        let clip = ClipRegion::from_path(
            2,
            2,
            &path,
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
            (0, 0),
        )
        .unwrap();
        let mut context = Context {
            overprint: crate::overprint::Policy::Ignore,
            elide_opaque_groups: false,
            separations: Arc::new(Separations::default()),
            preview: colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            raster: pagura_raster::Rasterizer::default(),
            bounds: regions::Prepared::new(Transform::IDENTITY, limits.decoded_bytes),
            fragments: None,
            spare: Vec::new(),
            palette: None,
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
            viewport: None,
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
            fragments: None,
            spare: Vec::new(),
            palette: None,
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
            viewport: None,
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
                &mut crate::display::GlyphHulls::default(),
            )
            .unwrap();
        }
        run
    }
    fn glyph_surface() -> Surface {
        Surface::new(
            Tile {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            vec![Pixel::source([0.3, 0.6, 0.9, 0.0], 1.0); 16],
        )
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
                    for (a, b) in actual.pixels().iter().zip(reference.pixels()) {
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
    #[test]
    fn narrow_rectangles_charge_their_region_as_masks() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 12,
            height: 10,
        };
        // The fill's region is 7 × 6 pixels at (2, 1); the crop leaves only
        // its first four columns covered, so the region holds zero samples.
        let mut path = PathBuilder::new();
        path.rect(2.5, 1.25, 6.0, 5.5);
        let paint = PathCommand {
            path: Arc::new(path.finish()),
            mark: Mark::Fill(FillRule::NonZero),
            clip: None,
            color: crate::color::Color::Solid {
                space: crate::color::Space::Rgb,
                components: vec![0.2, 0.5, 0.8],
            },
            position: pagura_core::Position::Unknown,
            transfer: None,
            composite: Composite::opaque(),
        };
        let context = |limits: Limits, work: usize| {
            let mut context = glyph_context(limits);
            let mut crop = PathBuilder::new();
            crop.rect(0.0, 0.0, 6.0, 10.0);
            context.crop = crop.finish();
            context.work = work;
            context
        };
        // `path_fill` charges only the visibility test. The closed form keeps
        // the region's area, although only 24 of its samples are covered.
        let mut probe = context(Limits::default(), 0);
        let Some(Coverage::Rectangle(rectangle)) = probe
            .path_fill(&paint, tile, pagura_raster::Rasterizer::clipped_coverage)
            .unwrap()
        else {
            panic!("the fill took the mask");
        };
        let covered: usize = rectangle.spans().map(|(_, _, run)| run.len()).sum();
        assert_eq!(
            (rectangle.origin(), rectangle.area(), covered),
            ((2, 1), 42, 24)
        );
        let used = probe.work + 42;
        // The closed form, and the dense mask path it replaces.
        let paint_with = |closed: bool, limits: Limits, work: usize| {
            let mut context = context(limits, work);
            let buffer = narrow::Buffer::new(plan::Format::N16, 120, [255; 4]).unwrap();
            let mut target = Surface::with_buffer(tile, buffer);
            let (initial, model) = (Backdrop::Transparent, Model::Rgb);
            let result = if closed {
                context.path_in((&paint, tile), &mut target, initial, model, false, 0)
            } else {
                let mask = context.path_mask(&paint, tile).unwrap().unwrap();
                let result =
                    context.paint_path((&paint, &mask), &mut target, initial, model, false, 0);
                context.raster.recycle(mask);
                result
            };
            let mut pixels = vec![0; 480];
            target.narrow().unwrap().output(0, &mut pixels);
            (result.map_err(|e| e.to_string()), context.work, pixels)
        };
        let (result, work, expected) = paint_with(false, Limits::default(), 0);
        assert_eq!((result, work), (Ok(()), used));
        let closed = paint_with(true, Limits::default(), 0);
        assert_eq!(closed, (Ok(()), used, expected.clone()));
        // At the compositing budget's boundary both paths succeed or fail alike.
        let limits = Limits {
            raster_work: 10_000,
            ..Limits::default()
        };
        for closed in [false, true] {
            let fits = paint_with(closed, limits, 10_000 - used);
            assert_eq!(fits, (Ok(()), 10_000, expected.clone()));
            let (result, ..) = paint_with(closed, limits, 10_000 - used + 1);
            let error = result.unwrap_err();
            assert!(
                error.contains("compositing work budget exceeded"),
                "{error}"
            );
        }
    }
}
