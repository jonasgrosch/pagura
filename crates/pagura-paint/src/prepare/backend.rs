//! What preparation asks of geometry and paint: shapes and clips from the
//! band rasterizer, prepared paint from the sources and colour conversions
//! from `pagura-color`, behind one seam so that preparation and execution
//! are tested with synthetic coverage and colours.

use crate::hash::{self, FastMap};
use crate::paint::{Convert, PaintSpans, StencilPaint};
use crate::prepare::bounds::{self, Hull};
use crate::source::{ImageBudgets, ImageSampler, PaintContext, Source};
use pagura_color::{ColorSpace, Conversion, Converter, Target};
use pagura_core::{Built, Limits, Meter, Result, Store, StoreKey};
use pagura_interpret::{GlyphSource, Image, OutlineKey, Paint, Pen, Shading};
use pagura_raster::{
    Area, BandCoverage, BandRasterizer, ClipShape, DeviceRect, FillRule, GlyphEdges, GlyphMask,
    GlyphMasks, GlyphOrigins, MarkCoverage, Path, Point, RunGlyph, Shape, ShapeOptions, Transform,
};
use std::fmt;
use std::sync::Arc;

/// A prepared shape or clip as preparation sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// Its index in the coverage producer.
    pub(crate) key: u32,
    /// The pixels it can cover or keep.
    pub(crate) bounds: DeviceRect,
    /// Rasterization work units at most (shapes only).
    pub(crate) work: usize,
    /// Bytes retained, counted once per prepared value.
    pub(crate) bytes: usize,
}

/// Paint preparation inputs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaintRequest<'a> {
    /// The context of `Source::prepare`.
    pub(crate) context: PaintContext<'a>,
    /// Device pixels the paint fills at most.
    pub(crate) pixels: usize,
}

/// Geometry and paint preparation for one render.
pub(crate) trait Backend {
    /// A fill.
    ///
    /// # Errors
    ///
    /// As `Shape::fill`.
    fn fill(
        &mut self,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>>;

    /// A stroke; `pen` maps user space to the space the line width is
    /// measured in.
    ///
    /// # Errors
    ///
    /// As `Shape::stroke`.
    fn stroke(
        &mut self,
        path: &Path,
        pen: (&Pen, Transform),
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>>;

    /// A glyph placement of `source` under the device linear map `linear`
    /// at the device `origin`.
    ///
    /// The second of `glyph` names the source's edges once a placement of
    /// it has found them, so that its other placements need not look them
    /// up again.
    ///
    /// # Errors
    ///
    /// As `GlyphEdges::new` and `Shape::glyph`.
    fn glyph(
        &mut self,
        glyph: (&GlyphSource, &mut Option<u32>),
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>>;

    /// The hull of `source`'s outline under the device linear map
    /// `transform` relative to a placement's origin, as [`bounds::path`]
    /// gives it. The second of `glyph` names the source's edges as in
    /// [`Self::glyph`]; a backend that finds them may take the hull they
    /// keep, and name them.
    fn glyph_hull(
        &mut self,
        glyph: (&GlyphSource, &mut Option<u32>),
        transform: Transform,
    ) -> Option<Hull> {
        bounds::path(&glyph.0.outline, transform)
    }

    /// The clip state the next shapes are prepared in.
    fn shapes_clip(&mut self, clip: Option<u32>) {
        let _ = clip;
    }

    /// A clip state: the intersection of `paths`, outermost first.
    ///
    /// # Errors
    ///
    /// As `ClipShape::new`.
    fn clip(
        &mut self,
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Prepared>;

    /// A solid, shading or image-free paint.
    ///
    /// # Errors
    ///
    /// As `Source::prepare`.
    fn paint(
        &mut self,
        paint: &Paint,
        request: &PaintRequest<'_>,
        functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>>;

    /// A shading painted with `sh`.
    ///
    /// # Errors
    ///
    /// As `ShadingSource::prepare`.
    fn shading(
        &mut self,
        shading: &Shading,
        to_device: Transform,
        request: &PaintRequest<'_>,
        functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>>;

    /// An image, or a stencil mask coloured by `stencil`, charging
    /// sampling to `render_work` and conversions to `function_work`.
    ///
    /// # Errors
    ///
    /// As `ImageSampler::prepare_with`.
    fn image(
        &mut self,
        image: &Image,
        to_device: Transform,
        stencil: Option<Box<dyn PaintSpans>>,
        request: &PaintRequest<'_>,
        budgets: (&mut Meter, &mut Meter),
    ) -> Result<Box<dyn PaintSpans>>;

    /// A conversion of `space` into `target`.
    ///
    /// # Errors
    ///
    /// As `Converter::new`.
    fn converter(
        &mut self,
        space: &ColorSpace,
        target: &Target,
        conversion: &Conversion,
    ) -> Result<Box<dyn Convert>>;

    /// The coverage producer of the prepared shapes and clips, its scratch
    /// storage charged to `bytes`.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the scratch storage exceeds `bytes`.
    fn finish(self, area: Area, bytes: &mut Meter) -> Result<Box<dyn Producer>>;
}

/// Coverage of prepared marks, band by band.
pub(crate) trait Producer: Send + Sync + fmt::Debug {
    /// Writes the coverage of `shape` (`None`: the whole area) within
    /// `clip` (`None`: everything) in `band` to `out`, replacing it.
    fn rasterize(
        &mut self,
        shape: Option<u32>,
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    );

    /// Writes the coverage within `band` of the leading `shapes`, which
    /// share no pixel and are all within `clip`, to `out`, replacing it:
    /// each pixel's coverage as [`Self::rasterize`] gives it for the one
    /// shape that can cover it, with zero coverage in the gaps of a row.
    /// Returns how many shapes it took, at least one when there are any.
    fn rasterize_run(
        &mut self,
        shapes: &[u32],
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) -> usize {
        if let Some(&shape) = shapes.first() {
            self.rasterize(Some(shape), clip, band, out);
            1
        } else {
            out.clear();
            0
        }
    }

    /// The mask of glyph placement `shape` when its coverage is a mask that
    /// stores as it is, its clip state keeping all of it.
    fn mask(&self, shape: u32) -> Option<GlyphMask<'_>> {
        let _ = shape;
        None
    }
}

/// The production backend: the band rasterizer and the paint sources.
#[derive(Debug)]
pub(crate) struct Raster<'a> {
    store: &'a Store,
    limits: &'a Limits,
    origins: GlyphOrigins,
    scale: f64,
    shapes: Vec<Shape>,
    /// Glyph masks, keyed with [`MASK`] set.
    masks: GlyphMasks,
    clips: Vec<ClipShape>,
    edges: FastMap<EdgesKey, Arc<GlyphEdges>>,
    /// Edges by the handles [`Backend::glyph`] gives out.
    handles: Vec<Arc<GlyphEdges>>,
    current: Option<u32>,
}

impl<'a> Raster<'a> {
    /// A backend that caches glyph edges in `store` and places glyphs at
    /// `origins`.
    pub(crate) fn new(store: &'a Store, limits: &'a Limits, origins: GlyphOrigins) -> Self {
        Self {
            store,
            limits,
            origins,
            scale: 1.0,
            shapes: SPARE_SHAPES
                .try_with(std::cell::Cell::take)
                .unwrap_or_default(),
            masks: SPARE_MASKS
                .try_with(std::cell::Cell::take)
                .unwrap_or_default(),
            clips: Vec::new(),
            edges: hash::map(),
            handles: Vec::new(),
            current: None,
        }
    }

    /// Device pixels per default user-space unit, for binned glyph edges.
    pub(crate) fn with_scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    /// The store key of `source`'s edges under the device linear map
    /// `linear`.
    fn edges_key(&self, source: &GlyphSource, linear: [f64; 4]) -> EdgesKey {
        let binned = (self.origins == GlyphOrigins::Binned).then_some(self.scale);
        EdgesKey {
            outline: source.key,
            linear: linear.map(f64::to_bits),
            binned: binned.map(f64::to_bits),
        }
    }

    fn shape(&mut self, shape: Option<Shape>) -> Result<Option<Prepared>> {
        let Some(shape) = shape else {
            return Ok(None);
        };
        let key = index(self.shapes.len())?;
        let prepared = Prepared {
            key,
            bounds: shape.bounds(),
            work: shape.work(),
            bytes: shape.retained_bytes(),
        };
        self.shapes.push(shape);
        Ok(Some(prepared))
    }
}

/// Glyph mask `index` of `masks` as prepared, keyed with [`MASK`] set.
fn mask_prepared(masks: &GlyphMasks, index: usize) -> Result<Option<Prepared>> {
    let key = self::index(index)? | MASK;
    Ok(masks.get(index).map(|mask| Prepared {
        key,
        bounds: mask.bounds(),
        work: mask.work(),
        bytes: mask.retained_bytes(),
    }))
}

/// The key bit of glyph masks, which the producer keeps apart from shapes.
const MASK: u32 = 1 << 31;

/// The identity of glyph edges in the [`Store`]: the outline and the bits
/// of its device linear map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgesKey {
    outline: OutlineKey,
    linear: [u64; 4],
    /// The device scale binned edges are flattened for; exact edges have none.
    binned: Option<u64>,
}

impl StoreKey for EdgesKey {
    type Value = GlyphEdges;
}

fn index(len: usize) -> Result<u32> {
    u32::try_from(len)
        .ok()
        .filter(|&i| i < MASK)
        .ok_or(pagura_core::Error::limit(
            "prepared shape count exceeds 2^31",
        ))
}

impl Backend for Raster<'_> {
    fn fill(
        &mut self,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let clip = self.current.and_then(|k| self.clips.get(k as usize));
        let shape = Shape::fill_with_clip(path, transform, rule, clip, options)?;
        self.shape(shape)
    }

    fn stroke(
        &mut self,
        path: &Path,
        (pen, inverse): (&Pen, Transform),
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let clip = self.current.and_then(|k| self.clips.get(k as usize));
        let shape = Shape::stroke_with_clip(
            path,
            &pen.stroke,
            (inverse, transform),
            pen.adjustment,
            clip,
            options,
        )?;
        self.shape(shape)
    }

    fn glyph(
        &mut self,
        (source, handle): (&GlyphSource, &mut Option<u32>),
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        if let Some(edges) = handle.and_then(|i| self.handles.get(i as usize)) {
            let clip = self.current.and_then(|k| self.clips.get(k as usize));
            if let Some(mask) = self.masks.place(edges, origin, clip, options)? {
                return mask_prepared(&self.masks, mask);
            }
            let shape = Shape::glyph_with_clip(edges, origin, self.origins, clip, options)?;
            return self.shape(shape);
        }
        let binned = (self.origins == GlyphOrigins::Binned).then_some(self.scale);
        let key = self.edges_key(source, linear);
        // Edges are a value of the store: they cost the render nothing,
        // found or built (ADR 0016), and are kept for the render's other
        // placements.
        let edges = if let Some(edges) = self.edges.get(&key) {
            Arc::clone(edges)
        } else {
            let limits = self.limits;
            let edges = self.store.get_or_build(key, || {
                let edges = match binned {
                    Some(scale) => GlyphEdges::binned(&source.outline, linear, scale, limits)?,
                    None => GlyphEdges::new(&source.outline, linear, limits)?,
                };
                let bytes = edges.retained_bytes();
                Ok(Built {
                    value: edges,
                    bytes,
                })
            })?;
            self.edges.insert(key, Arc::clone(&edges));
            edges
        };
        *handle = u32::try_from(self.handles.len()).ok();
        self.handles.push(Arc::clone(&edges));
        let clip = self.current.and_then(|k| self.clips.get(k as usize));
        if let Some(mask) = self.masks.place(&edges, origin, clip, options)? {
            return mask_prepared(&self.masks, mask);
        }
        let shape = Shape::glyph_with_clip(&edges, origin, self.origins, clip, options)?;
        self.shape(shape)
    }

    fn glyph_hull(
        &mut self,
        (source, handle): (&GlyphSource, &mut Option<u32>),
        transform: Transform,
    ) -> Option<Hull> {
        if let Some(edges) = handle.and_then(|i| self.handles.get(i as usize)) {
            return edges.hull();
        }
        // Edges kept from an earlier render keep the hull; edges are not
        // built before a placement meets the region.
        let key = self.edges_key(source, [transform.a, transform.b, transform.c, transform.d]);
        let Some(edges) = self
            .edges
            .get(&key)
            .cloned()
            .or_else(|| self.store.get(&key))
        else {
            return bounds::path(&source.outline, transform);
        };
        self.edges.entry(key).or_insert_with(|| Arc::clone(&edges));
        *handle = u32::try_from(self.handles.len()).ok();
        let hull = edges.hull();
        self.handles.push(edges);
        hull
    }

    fn shapes_clip(&mut self, clip: Option<u32>) {
        self.current = clip;
    }

    fn clip(
        &mut self,
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Prepared> {
        let clip = ClipShape::new(paths, transform, options)?;
        let key = index(self.clips.len())?;
        let prepared = Prepared {
            key,
            bounds: clip.bounds(),
            work: 0,
            bytes: clip.retained_bytes(),
        };
        self.clips.push(clip);
        Ok(prepared)
    }

    fn paint(
        &mut self,
        paint: &Paint,
        request: &PaintRequest<'_>,
        functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>> {
        let source = Source::prepare(paint, &request.context, request.pixels, functions)?;
        Ok(Box::new(source))
    }

    fn shading(
        &mut self,
        shading: &Shading,
        to_device: Transform,
        request: &PaintRequest<'_>,
        functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>> {
        Ok(Box::new(Source::shading(
            shading,
            to_device,
            &request.context,
            request.pixels,
            functions,
        )?))
    }

    fn image(
        &mut self,
        image: &Image,
        to_device: Transform,
        stencil: Option<Box<dyn PaintSpans>>,
        request: &PaintRequest<'_>,
        (work, functions): (&mut Meter, &mut Meter),
    ) -> Result<Box<dyn PaintSpans>> {
        let sampler = ImageSampler::prepare_with(
            image,
            to_device,
            &request.context,
            request.pixels,
            ImageBudgets { work, functions },
        )?;
        Ok(match stencil {
            // A stencil mask of one colour carries it in its texels, as the
            // exact route composites them.
            Some(color) => match color.constant() {
                Some(constant) => Box::new(Source::Image(
                    sampler.with_stencil_color(constant, color.painted()),
                )),
                None => Box::new(StencilPaint {
                    color,
                    mask: sampler,
                }),
            },
            None => Box::new(Source::Image(sampler)),
        })
    }

    fn converter(
        &mut self,
        space: &ColorSpace,
        target: &Target,
        conversion: &Conversion,
    ) -> Result<Box<dyn Convert>> {
        Ok(Box::new(Converter::new(space, target, conversion)?))
    }

    fn finish(self, area: Area, bytes: &mut Meter) -> Result<Box<dyn Producer>> {
        let spare = SPARE_RASTERIZER
            .try_with(std::cell::Cell::take)
            .ok()
            .flatten()
            .filter(|spare| spare.rasterizer.area() == area);
        let rasterizer = if let Some(spare) = spare {
            // The storage it holds charged as it did when it was made.
            bytes.charge(spare.charged)?;
            spare
        } else {
            let used = bytes.used();
            let rasterizer = BandRasterizer::new(area, bytes)?;
            Rasterizer {
                rasterizer,
                charged: bytes.used() - used,
            }
        };
        Ok(Box::new(RasterCoverage {
            rasterizer: Some(rasterizer),
            shapes: self.shapes,
            masks: self.masks,
            clips: self.clips,
        }))
    }
}

/// The production coverage producer.
#[derive(Debug)]
struct RasterCoverage {
    /// Taken back as the thread's spare when the producer drops.
    rasterizer: Option<Rasterizer>,
    shapes: Vec<Shape>,
    masks: GlyphMasks,
    clips: Vec<ClipShape>,
}

/// A band rasterizer and the bytes its storage charged.
#[derive(Debug)]
struct Rasterizer {
    rasterizer: BandRasterizer,
    charged: usize,
}

/// The shape or mask of `key`.
fn glyph<'a>(shapes: &'a [Shape], masks: &'a GlyphMasks, key: u32) -> Option<RunGlyph<'a>> {
    let get = |i: u32| pagura_core::convert::u32_to_usize(i);
    if key & MASK == 0 {
        shapes.get(get(key)).map(RunGlyph::Shape)
    } else {
        masks.get(get(key & !MASK)).map(RunGlyph::Mask)
    }
}

thread_local! {
    /// The emptied shape list of the coverage producer dropped last on this
    /// thread: the next preparation fills it without growing it again.
    static SPARE_SHAPES: std::cell::Cell<Vec<Shape>> = const { std::cell::Cell::new(Vec::new()) };
    /// Its emptied mask list, likewise.
    static SPARE_MASKS: std::cell::Cell<GlyphMasks> = std::cell::Cell::default();
    /// Its rasterizer, whose storage the next preparation of the same area
    /// takes rather than allocating and clearing it again.
    static SPARE_RASTERIZER: std::cell::Cell<Option<Rasterizer>> = const { std::cell::Cell::new(None) };
}

/// Keeps the emptied `list` as this thread's spare unless the spare holds
/// more without growing.
fn keep_spare<T: Default>(
    spare: &'static std::thread::LocalKey<std::cell::Cell<T>>,
    list: T,
    capacity: impl Fn(&T) -> usize,
) {
    // A thread being torn down has no spare to keep.
    let _ = spare.try_with(|spare| {
        let kept = spare.take();
        spare.set(if capacity(&kept) > capacity(&list) {
            kept
        } else {
            list
        });
    });
}

impl Drop for RasterCoverage {
    fn drop(&mut self) {
        let mut shapes = std::mem::take(&mut self.shapes);
        shapes.clear();
        keep_spare(&SPARE_SHAPES, shapes, Vec::capacity);
        let mut masks = std::mem::take(&mut self.masks);
        masks.clear();
        keep_spare(&SPARE_MASKS, masks, GlyphMasks::capacity);
        // Scratch that a panic left mid-mark is not taken again.
        if let Some(rasterizer) = self.rasterizer.take()
            && !std::thread::panicking()
        {
            let _ = SPARE_RASTERIZER.try_with(|spare| spare.set(Some(rasterizer)));
        }
    }
}

impl Producer for RasterCoverage {
    fn rasterize(
        &mut self,
        shape: Option<u32>,
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) {
        let Some(Rasterizer { rasterizer, .. }) = self.rasterizer.as_mut() else {
            out.clear();
            return;
        };
        let get = |i: u32| pagura_core::convert::u32_to_usize(i);
        let clip = clip.and_then(|i| self.clips.get(get(i)));
        let shape = match shape.map(|key| glyph(&self.shapes, &self.masks, key)) {
            Some(Some(RunGlyph::Mask(mask))) => {
                rasterizer.rasterize_mask(mask, band, out);
                return;
            }
            Some(Some(RunGlyph::Shape(shape))) => Some(shape),
            // A key no preparation gave out covers nothing.
            Some(None) => {
                out.clear();
                return;
            }
            None => None,
        };
        rasterizer.rasterize(MarkCoverage { shape, clip }, band, out);
    }

    fn rasterize_run(
        &mut self,
        shapes: &[u32],
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) -> usize {
        let Some(Rasterizer { rasterizer, .. }) = self.rasterizer.as_mut() else {
            out.clear();
            return 0;
        };
        let get = |i: u32| pagura_core::convert::u32_to_usize(i);
        let run: Vec<RunGlyph<'_>> = shapes
            .iter()
            .take(RUN_SHAPES)
            .map_while(|&key| glyph(&self.shapes, &self.masks, key))
            .collect();
        if run.is_empty() {
            out.clear();
            return 0;
        }
        let clip = clip.and_then(|i| self.clips.get(get(i)));
        rasterizer.rasterize_run(&run, clip, band, out)
    }

    fn mask(&self, shape: u32) -> Option<GlyphMask<'_>> {
        match glyph(&self.shapes, &self.masks, shape)? {
            RunGlyph::Mask(mask) => Some(mask),
            RunGlyph::Shape(_) => None,
        }
    }
}

/// The most shapes one call of [`Producer::rasterize_run`] passes on.
const RUN_SHAPES: usize = 256;
