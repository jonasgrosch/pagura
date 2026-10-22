//! What preparation asks of geometry and paint: shapes and clips from the
//! band rasterizer, prepared paint from the sources and colour conversions
//! from `pagura-color`, behind one seam so that preparation and execution
//! are tested with synthetic coverage and colours.

use crate::paint::{Convert, PaintSpans, StencilPaint};
use crate::source::{ImageBudgets, ImageSampler, PaintContext, Source};
use pagura_color::{ColorSpace, Conversion, Converter, Target};
use pagura_core::{Built, Limits, Meter, Result, Store, StoreKey};
use pagura_interpret::{GlyphSource, Image, OutlineKey, Paint, Pen, Shading};
use pagura_raster::{
    Area, BandCoverage, BandRasterizer, ClipShape, DeviceRect, FillRule, GlyphEdges, GlyphOrigins,
    MarkCoverage, Path, Point, Shape, ShapeOptions, Transform,
};
use crate::hash::{self, FastMap};
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
    /// # Errors
    ///
    /// As `GlyphEdges::new` and `Shape::glyph`.
    fn glyph(
        &mut self,
        source: &GlyphSource,
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>>;

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
}

/// The production backend: the band rasterizer and the paint sources.
#[derive(Debug)]
pub(crate) struct Raster<'a> {
    store: &'a Store,
    limits: &'a Limits,
    origins: GlyphOrigins,
    shapes: Vec<Shape>,
    clips: Vec<ClipShape>,
    edges: FastMap<EdgesKey, Arc<GlyphEdges>>,
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
            shapes: Vec::new(),
            clips: Vec::new(),
            edges: hash::map(),
            current: None,
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

/// The identity of glyph edges in the [`Store`]: the outline and the bits
/// of its device linear map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgesKey {
    outline: OutlineKey,
    linear: [u64; 4],
}

impl StoreKey for EdgesKey {
    type Value = GlyphEdges;
}

fn index(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| pagura_core::Error::limit("prepared shape count exceeds u32"))
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
        source: &GlyphSource,
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let key = EdgesKey {
            outline: source.key,
            linear: linear.map(f64::to_bits),
        };
        // Edges are a value of the store: they cost the render nothing,
        // found or built (ADR 0016), and are kept for the render's other
        // placements.
        let edges = if let Some(edges) = self.edges.get(&key) {
            Arc::clone(edges)
        } else {
            let limits = self.limits;
            let edges = self.store.get_or_build(key, || {
                let edges = GlyphEdges::new(&source.outline, linear, limits)?;
                let bytes = edges.retained_bytes();
                Ok(Built {
                    value: edges,
                    bytes,
                })
            })?;
            self.edges.insert(key, Arc::clone(&edges));
            edges
        };
        let clip = self.current.and_then(|k| self.clips.get(k as usize));
        let shape = Shape::glyph_with_clip(&edges, origin, self.origins, clip, options)?;
        self.shape(shape)
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
        let rasterizer = BandRasterizer::new(area, bytes)?;
        Ok(Box::new(RasterCoverage {
            rasterizer,
            shapes: self.shapes,
            clips: self.clips,
        }))
    }
}

/// The production coverage producer.
#[derive(Debug)]
struct RasterCoverage {
    rasterizer: BandRasterizer,
    shapes: Vec<Shape>,
    clips: Vec<ClipShape>,
}

impl Producer for RasterCoverage {
    fn rasterize(
        &mut self,
        shape: Option<u32>,
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) {
        let get = |i: u32| pagura_core::convert::u32_to_usize(i);
        let shape = shape.and_then(|i| self.shapes.get(get(i)));
        let clip = clip.and_then(|i| self.clips.get(get(i)));
        self.rasterizer
            .rasterize(MarkCoverage { shape, clip }, band, out);
    }
}
