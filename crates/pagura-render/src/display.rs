//! Immutable PDF display lists and tile composition (ADR 0004).

use crate::color::Color;
use crate::{RenderOptions, Tile};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::{Page, Rotation};
use pagura_raster::{
    ClipRegion, FillRule, Paint, Path, PathBuilder, Pixmap, Rasterizer, Stroke, Transform,
    adjusted_stroke_outline, stroke_outline,
};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone)]
pub(crate) struct Clip {
    pub(crate) parent: Option<Arc<Self>>,
    pub(crate) path: Arc<Path>,
    pub(crate) rule: FillRule,
}

#[derive(Debug)]
pub(crate) enum Mark {
    Fill(FillRule),
    Stroke(Stroke, Transform, bool),
}

#[derive(Debug)]
pub(crate) enum Command {
    Path(PathCommand),
    Glyphs(Box<GlyphRun>),
    Image(ImageCommand),
    Group(crate::transparency::Group),
    Shading(ShadingCommand),
}

#[derive(Debug)]
pub(crate) struct ImageCommand {
    pub(crate) image: Arc<crate::interpreter::images::Image>,
    pub(crate) transform: Transform,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) stencil: Color,
    pub(crate) composite: crate::transparency::Composite,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) position: pagura_core::Position,
}

#[derive(Debug)]
pub(crate) struct ShadingCommand {
    pub(crate) shading: Arc<crate::shading::Shading>,
    pub(crate) position: pagura_core::Position,
    pub(crate) transform: Transform,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) composite: crate::transparency::Composite,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) background: bool,
}

#[derive(Debug)]
pub(crate) struct PathCommand {
    // Construction CTMs have already mapped every path point into default
    // user space. A stroke's pen CTM is retained independently (8.5.2).
    pub(crate) path: Arc<Path>,
    pub(crate) mark: Mark,
    pub(crate) clip: Option<Arc<Clip>>,
    pub(crate) color: Color,
    pub(crate) position: pagura_core::Position,
    pub(crate) transfer: Option<crate::functions::Transfer>,
    pub(crate) composite: crate::transparency::Composite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    outline: usize,
    linear: [u64; 4],
}

/// The user-space and outline hulls of each glyph source met in one
/// compilation, so that later runs of the same glyph and linear map reuse
/// them. Each entry keeps its outline, so no key's address is reused.
#[derive(Debug, Default)]
pub(crate) struct GlyphHulls(
    crate::hash::FastMap<GlyphKey, (Arc<Path>, [[pagura_raster::Point; 2]; 2])>,
);

/// The user-space hull of `outline` under `linear` and the outline's own
/// hull, from every on- and off-curve point.
fn source_hulls(outline: &Path, linear: Transform) -> Result<[[pagura_raster::Point; 2]; 2]> {
    let mut lo = pagura_raster::Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut outline_lo = lo;
    let mut outline_hi = hi;
    let mut record = |p: pagura_raster::Point| -> Result<()> {
        outline_lo.x = outline_lo.x.min(p.x);
        outline_lo.y = outline_lo.y.min(p.y);
        outline_hi.x = outline_hi.x.max(p.x);
        outline_hi.y = outline_hi.y.max(p.y);
        let p = p * linear;
        if !p.is_finite() {
            return Err(Error::syntax("text matrix arithmetic overflow"));
        }
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
        Ok(())
    };
    for segment in outline.segments() {
        match *segment {
            pagura_raster::Segment::MoveTo(p) | pagura_raster::Segment::LineTo(p) => record(p)?,
            pagura_raster::Segment::CubicTo {
                c1: a,
                c2: b,
                end: c,
            } => {
                record(a)?;
                record(b)?;
                record(c)?;
            }
            pagura_raster::Segment::Close => {}
        }
    }
    Ok([[lo, hi], [outline_lo, outline_hi]])
}

#[derive(Debug)]
pub(crate) struct GlyphSource {
    pub(crate) outline: Arc<Path>,
    pub(crate) linear: Transform,
    hull: [pagura_raster::Point; 2],
    outline_hull: [pagura_raster::Point; 2],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Placement {
    pub(crate) source: u32,
    pub(crate) origin: pagura_raster::Point,
}

/// Every placement remains a separate mark, including overlapping pixels (9.3.8).
#[derive(Debug)]
pub(crate) struct GlyphRun {
    pub(crate) paint: PathCommand,
    pub(crate) sources: Vec<GlyphSource>,
    pub(crate) glyphs: Vec<Placement>,
    pub(crate) positions: Vec<(usize, pagura_core::Position)>,
    pub(crate) bounds: Envelope,
    origins: Envelope,
    index: crate::hash::FastMap<GlyphKey, u32>,
}

/// A running envelope of glyph points in user space. Unlike a [`Rect`] it
/// starts empty, inverted at infinity, and grows with each placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Envelope {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Envelope {
    const EMPTY: Self = Self {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };

    fn include(&mut self, lo: pagura_raster::Point, hi: pagura_raster::Point) {
        self.x0 = self.x0.min(lo.x);
        self.y0 = self.y0.min(lo.y);
        self.x1 = self.x1.max(hi.x);
        self.y1 = self.y1.max(hi.y);
    }

    pub(crate) const fn x0(&self) -> f64 {
        self.x0
    }

    pub(crate) const fn y0(&self) -> f64 {
        self.y0
    }

    pub(crate) const fn x1(&self) -> f64 {
        self.x1
    }

    pub(crate) const fn y1(&self) -> f64 {
        self.y1
    }
}

impl GlyphRun {
    /// Conservative capacity charge, including first-allocation minima and the
    /// hash index's estimated bookkeeping (ADR 0012).
    pub(crate) fn placement_bytes() -> usize {
        4 * (size_of::<Placement>()
            + size_of::<(usize, pagura_core::Position)>()
            + size_of::<GlyphSource>()
            + size_of::<GlyphKey>()
            + size_of::<u32>()
            + 64)
    }

    pub(crate) fn new(paint: PathCommand) -> Self {
        Self {
            paint,
            sources: Vec::new(),
            glyphs: Vec::new(),
            positions: Vec::new(),
            bounds: Envelope::EMPTY,
            origins: Envelope::EMPTY,
            index: crate::hash::FastMap::default(),
        }
    }
    pub(crate) fn push(
        &mut self,
        outline: Arc<Path>,
        matrix: Transform,
        position: pagura_core::Position,
        hulls: &mut GlyphHulls,
    ) -> Result<()> {
        let linear = Transform::new(matrix.a, matrix.b, matrix.c, matrix.d, 0.0, 0.0);
        let key = GlyphKey {
            outline: Arc::as_ptr(&outline) as usize,
            linear: [matrix.a, matrix.b, matrix.c, matrix.d].map(f64::to_bits),
        };
        let source = if let Some(&source) = self.index.get(&key) {
            source
        } else {
            let [hull, outline_hull] = if let Some((_, known)) = hulls.0.get(&key) {
                *known
            } else {
                let found = source_hulls(&outline, linear)?;
                hulls
                    .0
                    .try_reserve(1)
                    .map_err(|_| Error::limit("glyph hull allocation failed"))?;
                hulls.0.insert(key, (Arc::clone(&outline), found));
                found
            };
            let source = u32::try_from(self.sources.len())
                .map_err(|_| Error::limit("glyph source count exceeded"))?;
            self.sources
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph source allocation failed"))?;
            self.index
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph index allocation failed"))?;
            self.sources.push(GlyphSource {
                outline,
                linear,
                hull,
                outline_hull,
            });
            self.index.insert(key, source);
            source
        };
        let placement = Placement {
            source,
            origin: pagura_raster::Point::new(matrix.e, matrix.f),
        };
        let [lo, hi] = self.sources
            [usize::try_from(source).map_err(|_| Error::limit("glyph index exceeds usize"))?]
        .user_hull(placement.origin);
        if !lo.is_finite() || !hi.is_finite() {
            return Err(Error::syntax("path arithmetic overflow"));
        }
        self.origins.include(placement.origin, placement.origin);
        self.bounds.include(lo, hi);
        if self.positions.last().is_none_or(|(_, p)| *p != position) {
            self.positions
                .try_reserve(1)
                .map_err(|_| Error::limit("glyph position allocation failed"))?;
            self.positions.push((self.glyphs.len(), position));
        }
        self.glyphs
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph placement allocation failed"))?;
        self.glyphs.push(placement);
        Ok(())
    }
    pub(crate) fn bounds_work(&self) -> Result<usize> {
        self.sources
            .len()
            .checked_mul(4)
            .and_then(|n| n.checked_add(4))
            .ok_or(Error::limit("glyph bounds work overflow"))
    }
    /// Bound relative geometry and device origins separately. Forming a user
    /// path first can erase small extents beside a large origin (ADR 0013).
    pub(crate) fn device_hull(&self, device: Transform) -> [pagura_raster::Point; 2] {
        let origins = point_hull(
            corners([
                pagura_raster::Point::new(self.origins.x0(), self.origins.y0()),
                pagura_raster::Point::new(self.origins.x1(), self.origins.y1()),
            ])
            .map(|p| p * device),
        );
        let relative = point_hull(self.sources.iter().flat_map(|s| s.relative_hull(device)));
        point_hull([
            pagura_raster::Point::new(relative[0].x + origins[0].x, relative[0].y + origins[0].y),
            pagura_raster::Point::new(relative[1].x + origins[1].x, relative[1].y + origins[1].y),
        ])
    }
    pub(crate) fn position(&self, index: usize) -> pagura_core::Position {
        let count = self.positions.partition_point(|(first, _)| *first <= index);
        count
            .checked_sub(1)
            .and_then(|i| self.positions.get(i))
            .map_or(self.paint.position, |(_, p)| *p)
    }
    pub(crate) fn source(&self, placement: Placement) -> Result<&GlyphSource> {
        self.sources
            .get(
                usize::try_from(placement.source)
                    .map_err(|_| Error::limit("glyph index exceeds usize"))?,
            )
            .ok_or(Error::invalid_argument("invalid glyph source"))
    }
    pub(crate) fn geometry(
        &self,
        placement: Placement,
        device: Transform,
        limits: &Limits,
    ) -> Result<Arc<pagura_raster::GlyphGeometry>> {
        let source = self.source(placement)?;
        let d = Transform::new(device.a, device.b, device.c, device.d, 0.0, 0.0);
        let l = source.linear.then(d);
        Ok(Arc::new(pagura_raster::GlyphGeometry::new(
            &source.outline,
            [l.a, l.b, l.c, l.d],
            limits,
        )?))
    }
    fn rasterize(
        &self,
        target: &mut Pixmap,
        device: Transform,
        clip: &ClipRegion,
        raster: &mut Rasterizer,
        cache: &mut GlyphCache,
        limits: Limits,
    ) -> Result<()> {
        let paint = Paint::Solid(self.paint.color.to_rgba(
            self.paint.transfer.as_ref(),
            self.paint.composite.conversion.intent,
        )?);
        for (index, &placement) in self.glyphs.iter().enumerate() {
            let geometry = cache
                .geometry(self, placement, device, &limits, raster)
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
                .fill_glyph(target, geometry, placement.origin * device, &paint, clip)
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
impl GlyphSource {
    fn user_hull(&self, origin: pagura_raster::Point) -> [pagura_raster::Point; 2] {
        let transform = Transform::translate(origin.x, origin.y);
        self.hull.map(|p| p * transform)
    }
    pub(crate) fn device_hull(
        &self,
        origin: pagura_raster::Point,
        device: Transform,
    ) -> [pagura_raster::Point; 2] {
        let relative = self.relative_hull(device);
        let origin = origin * device;
        point_hull(relative.map(|p| pagura_raster::Point::new(p.x + origin.x, p.y + origin.y)))
    }
    /// The device hull of the glyph at a device origin of zero.
    pub(crate) fn relative_hull(&self, device: Transform) -> [pagura_raster::Point; 2] {
        let linear = self.linear.then(Transform::new(
            device.a, device.b, device.c, device.d, 0.0, 0.0,
        ));
        point_hull(corners(self.outline_hull).map(|p| p * linear))
    }
}
fn corners([lo, hi]: [pagura_raster::Point; 2]) -> [pagura_raster::Point; 4] {
    [
        lo,
        hi,
        pagura_raster::Point::new(lo.x, hi.y),
        pagura_raster::Point::new(hi.x, lo.y),
    ]
}
fn point_hull(points: impl IntoIterator<Item = pagura_raster::Point>) -> [pagura_raster::Point; 2] {
    let mut lo = pagura_raster::Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in points {
        // Loose box corners can overflow even if all actual source controls
        // remain finite. An unbounded allocation hint leaves validation to the
        // canonical geometry and avoids culling valid correlated controls.
        if p.x.is_nan() || p.y.is_nan() {
            return [
                pagura_raster::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
                pagura_raster::Point::new(f64::INFINITY, f64::INFINITY),
            ];
        }
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
    }
    [lo, hi]
}

#[derive(Debug)]
struct CachedGeometry {
    _outline: Arc<Path>,
    geometry: Arc<pagura_raster::GlyphGeometry>,
}

/// Optional per-render working memory; neither output nor work limits use hits.
#[derive(Debug, Default)]
pub(crate) struct GlyphCache {
    entries: crate::hash::FastMap<GlyphKey, CachedGeometry>,
    bytes: usize,
    maximum: usize,
}
impl GlyphCache {
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            maximum: (bytes / 16).min(16 * 1024 * 1024),
            ..Self::default()
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }
    pub(crate) fn clear(&mut self) {
        let maximum = self.maximum;
        *self = Self {
            maximum,
            ..Self::default()
        };
    }
    pub(crate) fn geometry(
        &mut self,
        run: &GlyphRun,
        placement: Placement,
        device: Transform,
        limits: &Limits,
        raster: &mut Rasterizer,
    ) -> Result<Arc<pagura_raster::GlyphGeometry>> {
        let source = run.source(placement)?;
        let linear = source.linear.then(Transform::new(
            device.a, device.b, device.c, device.d, 0.0, 0.0,
        ));
        let key = GlyphKey {
            outline: Arc::as_ptr(&source.outline) as usize,
            linear: [linear.a, linear.b, linear.c, linear.d].map(f64::to_bits),
        };
        if let Some(entry) = self.entries.get(&key)
            && entry.geometry.retained_bytes() <= limits.decoded_bytes
        {
            return Ok(Arc::clone(&entry.geometry));
        }
        let resident = self.bytes.saturating_add(raster.retained_bytes());
        let budget = Limits {
            decoded_bytes: limits.decoded_bytes.saturating_sub(resident),
            ..*limits
        };
        let geometry = match run.geometry(placement, device, &budget) {
            Err(error) if resident != 0 && error.kind == pagura_core::ErrorKind::Limit => {
                self.clear();
                raster.clear();
                run.geometry(placement, device, limits)?
            }
            result => result?,
        };
        let bytes = geometry
            .retained_bytes()
            .saturating_add(size_of::<GlyphKey>())
            .saturating_add(size_of::<CachedGeometry>())
            .saturating_add(64);
        let maximum = self.maximum.min(
            limits
                .decoded_bytes
                .saturating_sub(geometry.retained_bytes()),
        );
        if bytes > 64 * 1024
            || bytes > maximum
            || geometry.bounds().is_some_and(|bounds| {
                bounds
                    .iter()
                    .any(|p| p.x.abs().max(p.y.abs()) > f64::from(1_u32 << 30))
            })
        {
            return Ok(geometry);
        }
        if self.bytes.saturating_add(bytes) > maximum {
            self.clear();
        }
        let capacity = self.entries.capacity();
        if self.entries.try_reserve(1).is_ok() {
            let index_bytes = (self.entries.capacity() - capacity)
                .saturating_mul(size_of::<GlyphKey>() + size_of::<CachedGeometry>() + 64);
            let projected = self
                .bytes
                .saturating_add(geometry.retained_bytes())
                .saturating_add(16)
                .saturating_add(index_bytes);
            if projected > maximum {
                self.clear();
                return Ok(geometry);
            }
            self.entries.insert(
                key,
                CachedGeometry {
                    _outline: Arc::clone(&source.outline),
                    geometry: Arc::clone(&geometry),
                },
            );
            self.bytes = projected;
        }
        Ok(geometry)
    }
}

/// An immutable interpreted page, retaining source colours and default-user-space
/// geometry separately from device pixels (ISO 32000-2, 8.4–8.6; ADR 0004).
///
/// Compile with [`crate::compile_page`]; [`crate::render_page`] and
/// [`crate::render_tile`] compile and render in one call.
#[derive(Debug)]
pub struct DisplayList {
    pub(crate) commands: Vec<Command>,
    pub(crate) crop: Rect,
    pub(crate) rotation: Rotation,
    pub(crate) user_unit: f64,
    pub(crate) limits: Limits,
    pub(crate) group: crate::transparency::Attributes,
    pub(crate) print_intent: bool,
    pub(crate) separations: OnceLock<Result<Arc<crate::compositor::Separations>>>,
}

impl DisplayList {
    pub(crate) fn new(page: &Page, limits: Limits) -> Self {
        Self {
            commands: Vec::new(),
            crop: page.crop_box(),
            rotation: page.rotation(),
            user_unit: page.user_unit(),
            limits,
            group: crate::transparency::Attributes::default(),
            print_intent: false,
            separations: OnceLock::new(),
        }
    }

    pub(crate) fn separations(&self) -> Result<Arc<crate::compositor::Separations>> {
        self.separations
            .get_or_init(|| {
                crate::compositor::Separations::new(&self.commands, self.limits).map(Arc::new)
            })
            .clone()
    }

    pub(crate) fn preview_separations(
        &self,
        options: &RenderOptions,
    ) -> Result<Arc<crate::compositor::Separations>> {
        if options.overprint.resolve(self.print_intent) == crate::overprint::Policy::Simulate {
            self.separations()
        } else {
            Ok(Arc::default())
        }
    }

    /// Number of painted marks (a fill-and-stroke operation produces two).
    pub fn len(&self) -> usize {
        count_marks(&self.commands)
    }

    /// Number of glyph placements among the marks.
    pub(crate) fn glyph_placements(&self) -> usize {
        count_glyphs(&self.commands)
    }

    /// Whether the page contains no painted marks.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Conservative bounds of painted content in default user space, clamped to
    /// the crop box (12.3.2.2). Returns `None` for empty or disjoint culling geometry.
    ///
    /// Uses the renderer's culling geometry: cubic control hulls, stroke
    /// envelopes, group bounds and hard-clip envelopes. These can include empty
    /// space; the result is not a tight geometric intersection. Unbounded
    /// shadings use the crop. Point contours and hairlines use a one-unit
    /// reference device; annotation view flags introduce no zoom or rotation.
    ///
    /// # Errors
    /// Returns `Limit` for overflowing geometry or exhausted traversal budgets.
    pub fn content_bounds(&self) -> Result<Option<Rect>> {
        crate::compositor::content_bounds(self)
    }

    /// Device size after crop, rotation, user unit and scale (8.3.2).
    ///
    /// # Errors
    /// Returns `InvalidArgument` for invalid scales and `Limit` for overflow.
    pub fn device_size(&self, options: &RenderOptions) -> Result<(u32, u32)> {
        crate::device_extent(self.crop, self.rotation, self.user_unit, options)
    }

    /// Renders the complete display list into premultiplied RGBA8.
    ///
    /// # Errors
    /// Returns scale, raster geometry and resource-limit errors. No partial
    /// surface is returned on failure.
    pub fn render(&self, options: &RenderOptions) -> Result<Pixmap> {
        let (width, height) = self.device_size(options)?;
        self.render_tile(
            options,
            Tile {
                x: 0,
                y: 0,
                width,
                height,
            },
        )
    }

    /// Renders a device tile; any partition assembles byte-for-byte into
    /// [`Self::render`]. Integer tile origins select global device pixels without altering flattening.
    ///
    /// # Errors
    /// As [`Self::render`], plus `InvalidArgument` for tiles outside the page.
    pub fn render_tile(&self, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        self.render_tile_with(options, tile, crate::compositor::Lent::default())
    }

    /// As [`Self::render_tile`], with storage a rendering context lends to
    /// the competitive path.
    pub(crate) fn render_tile_with(
        &self,
        options: &RenderOptions,
        tile: Tile,
        lent: crate::compositor::Lent<'_>,
    ) -> Result<Pixmap> {
        let (width, height) = self.device_size(options)?;
        if tile.width == 0
            || tile.height == 0
            || tile.x.checked_add(tile.width).is_none_or(|n| n > width)
            || tile.y.checked_add(tile.height).is_none_or(|n| n > height)
        {
            return Err(Error::invalid_argument("tile lies outside the page"));
        }
        if options.accuracy == crate::Accuracy::Competitive {
            match crate::compositor::render_competitive(self, options, tile, lent) {
                // A limit on the competitive path must not make a page fail
                // that the exact path renders within its limits. Any limit
                // error falls back, so a rendering costs at most one
                // competitive attempt plus the exact rendering.
                Err(error) if error.kind == pagura_core::ErrorKind::Limit => {}
                result => return result,
            }
        }
        if self.preview_separations(options)?.enabled
            || self.group.knockout
            || self
                .group
                .model
                .is_some_and(|m| m != crate::transparency::Model::Rgb)
            || !opaque_commands(&self.commands)
        {
            return crate::compositor::render(self, options, tile);
        }
        self.render_opaque(options, tile)
    }

    fn render_opaque(&self, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        let device = self.device_transform(options);
        let mut target =
            Pixmap::new_with_limits(tile.width, tile.height, options.background, self.limits)?;
        // The crop is a geometric clip, including fractional final rows/columns.
        let mut crop = PathBuilder::new();
        crop.rect(
            self.crop.x0(),
            self.crop.y0(),
            self.crop.width(),
            self.crop.height(),
        );
        let crop = ClipRegion::from_path(
            tile.width,
            tile.height,
            &crop.finish(),
            device,
            FillRule::NonZero,
            self.limits,
            (tile.x, tile.y),
        )?;
        let mut raster = Rasterizer::default();
        let mut glyph_cache = GlyphCache::new(self.limits.decoded_bytes);
        let mut last_clip = None;
        let mut clip_region = crop.clone();
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
                clip_region = crop.clone();
                let mut clip = command.clip.as_deref();
                let mut chain = Vec::new();
                while let Some(node) = clip {
                    chain.push(node);
                    clip = node.parent.as_deref();
                }
                for node in chain.into_iter().rev() {
                    let next = ClipRegion::from_path(
                        tile.width,
                        tile.height,
                        &node.path,
                        device,
                        node.rule,
                        self.limits,
                        (tile.x, tile.y),
                    )?;
                    clip_region.intersect(&next)?;
                }
                last_clip = identity;
            }
            let result = if let Command::Glyphs(run) = original {
                run.rasterize(
                    &mut target,
                    device,
                    &clip_region,
                    &mut raster,
                    &mut glyph_cache,
                    self.limits,
                )
            } else {
                command.rasterize(&mut target, device, &clip_region, &mut raster, self.limits)
            };
            result.map_err(|e| contextual(e, command.position))?;
        }
        Ok(target)
    }

    pub(crate) fn device_transform(&self, options: &RenderOptions) -> Transform {
        let r = self.crop;
        let rotation = match self.rotation {
            Rotation::None => Transform::new(1.0, 0.0, 0.0, -1.0, -r.x0(), r.y1()),
            Rotation::Clockwise90 => Transform::new(0.0, 1.0, 1.0, 0.0, -r.y0(), -r.x0()),
            Rotation::Clockwise180 => Transform::new(-1.0, 0.0, 0.0, 1.0, r.x1(), -r.y0()),
            Rotation::Clockwise270 => Transform::new(0.0, -1.0, -1.0, 0.0, r.y1(), r.x1()),
        };
        rotation.then(Transform::scale(
            options.scale * self.user_unit,
            options.scale * self.user_unit,
        ))
    }
}

fn count_glyphs(commands: &[Command]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            Command::Path(_) | Command::Shading(_) | Command::Image(_) => 0,
            Command::Group(group) => count_glyphs(&group.commands),
            Command::Glyphs(run) => run.glyphs.len(),
        })
        .sum()
}

fn count_marks(commands: &[Command]) -> usize {
    commands
        .iter()
        .map(|command| match command {
            Command::Path(_) | Command::Shading(_) | Command::Image(_) => 1,
            Command::Group(group) => count_marks(&group.commands),
            Command::Glyphs(run) => run.glyphs.len(),
        })
        .sum()
}

impl PathCommand {
    fn rasterize(
        &self,
        target: &mut Pixmap,
        device: Transform,
        clip: &ClipRegion,
        raster: &mut Rasterizer,
        limits: Limits,
    ) -> Result<()> {
        let paint = Paint::Solid(
            self.color
                .to_rgba(self.transfer.as_ref(), self.composite.conversion.intent)?,
        );
        match &self.mark {
            Mark::Fill(rule) => {
                raster.fill_path(target, Arc::clone(&self.path), device, *rule, &paint, clip)?;
            }
            Mark::Stroke(stroke, ctm, adjust) => {
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
                let (path, transform) = outline(&path, ctm.then(device), stroke, &limits)?;
                raster.fill_path(
                    target,
                    Arc::new(path),
                    transform,
                    FillRule::NonZero,
                    &paint,
                    clip,
                )?;
            }
        }
        Ok(())
    }
}

/// Opaque Normal non-knockout RGB groups are source-over associative
/// (11.3.3, 11.4.5); no intermediate surface or colour conversion is needed.
fn opaque_commands(commands: &[Command]) -> bool {
    commands.iter().all(|command| match command {
        Command::Glyphs(run) => {
            run.paint.composite.simple() && matches!(run.paint.color, Color::Solid { .. })
        }
        Command::Path(p) => p.composite.simple() && matches!(p.color, Color::Solid { .. }),
        Command::Group(g) => {
            g.composite.simple()
                && g.attributes.view.is_none()
                && !g.attributes.knockout
                && (!g.attributes.isolated
                    || g.attributes
                        .model
                        .is_none_or(|m| m == crate::transparency::Model::Rgb))
                && opaque_commands(&g.commands)
        }
        Command::Shading(_) | Command::Image(_) => false,
    })
}

pub(crate) fn contextual(error: Error, position: pagura_core::Position) -> Error {
    let error = error.at(position);
    if let pagura_core::Position::Stream { object, .. } = position {
        error.in_object(object)
    } else {
        error
    }
}
impl Command {
    pub(crate) fn position(&self) -> pagura_core::Position {
        match self {
            Self::Path(p) => p.position,
            Self::Glyphs(run) => run.paint.position,
            Self::Image(i) => i.position,
            Self::Shading(s) => s.position,
            Self::Group(g) => g.position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::Position;
    use pagura_raster::Point;

    fn paint(alpha: f64) -> PathCommand {
        PathCommand {
            path: Arc::default(),
            mark: Mark::Fill(FillRule::NonZero),
            clip: None,
            color: Color::default(),
            position: Position::Unknown,
            transfer: None,
            composite: crate::transparency::Composite {
                alpha,
                ..Default::default()
            },
        }
    }
    fn run(width: f64, origins: &[f64], alpha: f64) -> GlyphRun {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, width, 1.0);
        let path = Arc::new(path.finish());
        let mut run = GlyphRun::new(paint(alpha));
        for &x in origins {
            run.push(
                Arc::clone(&path),
                Transform::translate(x, 0.0),
                Position::Unknown,
                &mut GlyphHulls::default(),
            )
            .unwrap();
        }
        run
    }
    fn list(run: GlyphRun, knockout: bool) -> DisplayList {
        let commands = vec![Command::Glyphs(Box::new(run))];
        let commands = if knockout {
            vec![Command::Group(crate::transparency::Group {
                position: Position::Unknown,
                commands,
                bounds: None,
                attributes: crate::transparency::Attributes {
                    knockout: true,
                    ..Default::default()
                },
                composite: crate::transparency::Composite::opaque(),
            })]
        } else {
            commands
        };
        DisplayList {
            commands,
            crop: Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap(),
            rotation: Rotation::None,
            user_unit: 1.0,
            limits: Limits::default(),
            group: crate::transparency::Attributes::default(),
            print_intent: false,
            separations: OnceLock::new(),
        }
    }
    #[test]
    fn device_bounds_preserve_small_extents_at_large_user_origins() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 0.4, 1.0);
        let mut run = GlyphRun::new(paint(1.0));
        run.push(
            Arc::new(path.finish()),
            Transform::new(1e-5, 0.0, 0.0, 1e-5, 5e11, 0.0),
            Position::Unknown,
            &mut GlyphHulls::default(),
        )
        .unwrap();
        let device = Transform::new(1e6, 0.0, 0.0, 1e6, -5e17, 0.0);
        let placement = run.glyphs[0];
        // L = 10I and t = 0. Adding the user origin first erases the width.
        let hull = run
            .source(placement)
            .unwrap()
            .device_hull(placement.origin, device);
        assert!(hull[0].x <= 0.0 && hull[1].x >= 4.0);
        assert!(hull[0].y <= 0.0 && hull[1].y >= 10.0);
        let run_hull = run.device_hull(device);
        assert!(run_hull[0].x <= 0.0 && run_hull[1].x >= 4.0);
        let mut list = list(run, true);
        list.crop = Rect::from_corners(5e11, 0.0, 5e11 + 0.001, 0.001).unwrap();
        let image = crate::compositor::render(
            &list,
            &RenderOptions::from_dpi(72e6),
            Tile {
                x: 2,
                y: 995,
                width: 1,
                height: 1,
            },
        )
        .unwrap();
        // The canonical rectangle covers this whole pixel; the backdrop is white.
        assert_eq!(image.data(), [0, 0, 0, 255]);
    }

    #[test]
    fn shared_columns_are_individual_marks_in_content_order() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        for (width, origins, alpha, knockout, expected) in [
            (0.4, [0.0, 0.6], 1.0, false, 92),
            (0.6, [0.2, 0.2], 1.0, false, 41),
            (0.4, [0.0, 0.6], 0.5, false, 163),
            (0.4, [0.0, 0.6], 0.5, true, 173),
        ] {
            let list = list(run(width, &origins, alpha), knockout);
            assert_eq!(list.len(), 2);
            let image = crate::compositor::render(&list, &RenderOptions::default(), tile).unwrap();
            assert_eq!(&image.data()[..4], &[expected, expected, expected, 255]);
            for x in 0..2 {
                let part = crate::compositor::render(
                    &list,
                    &RenderOptions::default(),
                    Tile {
                        x,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                )
                .unwrap();
                assert_eq!(
                    part.data(),
                    &image.data()
                        [usize::try_from(x).unwrap() * 4..usize::try_from(x + 1).unwrap() * 4]
                );
            }
        }
    }
    #[test]
    fn cache_hits_eviction_and_disabled_cache_keep_geometry_and_limits() {
        let run = run(0.4, &[0.0, 0.6], 1.0);
        let limits = Limits::default();
        let mut cache = GlyphCache::new(limits.decoded_bytes);
        let mut raster = Rasterizer::default();
        let first = cache
            .geometry(
                &run,
                run.glyphs[0],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            )
            .unwrap();
        let second = cache
            .geometry(
                &run,
                run.glyphs[1],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(cache.retained_bytes() <= cache.maximum);
        cache.clear();
        cache.maximum = 0;
        let cold = cache
            .geometry(
                &run,
                run.glyphs[0],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            )
            .unwrap();
        assert_eq!(first, cold);
        assert_eq!(cache.retained_bytes(), 0);
        for bytes in [
            first.retained_bytes() - 1,
            first.retained_bytes(),
            limits.decoded_bytes,
        ] {
            let limits = Limits {
                decoded_bytes: bytes,
                ..limits
            };
            let cold = cache.geometry(
                &run,
                run.glyphs[0],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            );
            cache.maximum = limits.decoded_bytes;
            let warm = cache.geometry(
                &run,
                run.glyphs[0],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            );
            assert_eq!(
                cold.as_ref().map(|g| g.contours()).map_err(|e| e.kind),
                warm.as_ref().map(|g| g.contours()).map_err(|e| e.kind)
            );
            cache.clear();
        }
        assert_eq!(size_of::<Placement>(), 24);
    }
    #[test]
    fn geometry_keys_preserve_signed_zero_in_the_linear_map() {
        let mut run = run(0.4, &[0.0], 1.0);
        let outline = Arc::clone(&run.sources[0].outline);
        let signed = Transform::new(1.0, -0.0, -0.0, 1.0, 0.0, 0.0);
        run.push(
            outline,
            signed,
            Position::Unknown,
            &mut GlyphHulls::default(),
        )
        .unwrap();
        assert_ne!(run.glyphs[0].source, run.glyphs[1].source);
        let limits = Limits::default();
        let mut cache = GlyphCache::new(limits.decoded_bytes);
        let mut raster = Rasterizer::default();
        let positive = cache
            .geometry(
                &run,
                run.glyphs[1],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            )
            .unwrap();
        let negative = cache
            .geometry(&run, run.glyphs[1], signed, &limits, &mut raster)
            .unwrap();
        assert!(!Arc::ptr_eq(&positive, &negative));
        let repeated = cache
            .geometry(&run, run.glyphs[1], signed, &limits, &mut raster)
            .unwrap();
        assert!(Arc::ptr_eq(&negative, &repeated));
    }
    #[test]
    fn geometry_keys_distinguish_linear_maps_and_keep_outline_identities_alive() {
        let first_run = run(0.4, &[0.0], 1.0);
        let limits = Limits::default();
        let outline = Arc::downgrade(&first_run.sources[0].outline);
        let mut cache = GlyphCache::new(limits.decoded_bytes);
        let mut raster = Rasterizer::default();
        let first = cache
            .geometry(
                &first_run,
                first_run.glyphs[0],
                Transform::IDENTITY,
                &limits,
                &mut raster,
            )
            .unwrap();
        let scaled = cache
            .geometry(
                &first_run,
                first_run.glyphs[0],
                Transform::scale(-2.0, 1.0),
                &limits,
                &mut raster,
            )
            .unwrap();
        assert_ne!(first, scaled);
        drop(first_run);
        assert!(outline.upgrade().is_some());
        cache.clear();
        assert!(outline.upgrade().is_none());
        assert_eq!(first.contours()[0].points()[1], Point::new(0.4, 0.0));
    }
}
