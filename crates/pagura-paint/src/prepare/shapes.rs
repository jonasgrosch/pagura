//! Coverage sources (ADR 0034 §4): the shapes and clips preparation builds
//! with the band rasterizer, and their coverage band by band.

use crate::plan::Cover;
use crate::prepare::bounds::{self, Hull};
use pagura_core::convert::u32_to_usize;
use pagura_core::hash::FastMap;
use pagura_core::{Built, Limits, Result, Store, StoreKey};
use pagura_interpret::{GlyphSource, OutlineKey, Pen};
use pagura_raster::{
    BandCoverage, BandRasterizer, ClipShape, DeviceRect, FillRule, GlyphEdges, MarkCoverage, Path,
    Point, Shape, ShapeOptions, Transform,
};
use std::sync::Arc;

/// A prepared shape or clip as preparation sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// Its index among the shapes or clips.
    pub(crate) key: u32,
    /// The pixels it can cover or keep.
    pub(crate) bounds: DeviceRect,
    /// Rasterization work units at most (shapes only).
    pub(crate) work: usize,
    /// Bytes retained, counted once per prepared value.
    pub(crate) bytes: usize,
}

/// The storage prepared shapes are kept in.
pub(crate) type Storage = Vec<Shape>;

/// The shapes and clips of one preparation, with the glyph edges its
/// placements share.
#[derive(Debug)]
pub(crate) struct ShapeBuilder<'a> {
    store: &'a Store,
    limits: &'a Limits,
    shapes: Vec<Shape>,
    clips: Vec<ClipShape>,
    edges: FastMap<EdgesKey, Arc<GlyphEdges>>,
    /// Edges by the handles [`Self::glyph`] gives out.
    handles: Vec<Arc<GlyphEdges>>,
    /// The clip state shapes are prepared in.
    clip: Option<u32>,
}

/// The identity of glyph edges in the [`Store`]: the outline and the bits
/// of its device linear map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgesKey {
    outline: OutlineKey,
    linear: [u64; 4],
}

impl EdgesKey {
    /// The key of `source`'s edges under the device linear map `linear`.
    fn new(source: &GlyphSource, linear: [f64; 4]) -> Self {
        Self {
            outline: source.key,
            linear: linear.map(f64::to_bits),
        }
    }
}

impl StoreKey for EdgesKey {
    type Value = GlyphEdges;
}

fn index(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| pagura_core::Error::limit("prepared shape count exceeds u32"))
}

impl<'a> ShapeBuilder<'a> {
    /// Shapes kept in `storage`, emptied, with glyph edges cached in
    /// `store`.
    pub(crate) fn new(store: &'a Store, limits: &'a Limits, mut storage: Storage) -> Self {
        storage.clear();
        Self {
            store,
            limits,
            shapes: storage,
            clips: Vec::new(),
            edges: FastMap::default(),
            handles: Vec::new(),
            clip: None,
        }
    }

    /// Prepares the next shapes within clip `clip` (`None`: unclipped).
    pub(crate) fn set_clip(&mut self, clip: Option<u32>) {
        self.clip = clip;
    }

    fn current_clip(&self) -> Option<&ClipShape> {
        self.clip.and_then(|k| self.clips.get(u32_to_usize(k)))
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

    /// A fill.
    ///
    /// # Errors
    ///
    /// As `Shape::fill_with_clip`.
    pub(crate) fn fill(
        &mut self,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let shape = Shape::fill_with_clip(path, transform, rule, self.current_clip(), options)?;
        self.shape(shape)
    }

    /// A stroke; `inverse` maps user space to the space the pen's line
    /// width is measured in.
    ///
    /// # Errors
    ///
    /// As `Shape::stroke_with_clip`.
    pub(crate) fn stroke(
        &mut self,
        path: &Path,
        (pen, inverse): (&Pen, Transform),
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let shape = Shape::stroke_with_clip(
            path,
            &pen.stroke,
            (inverse, transform),
            pen.adjustment,
            self.current_clip(),
            options,
        )?;
        self.shape(shape)
    }

    /// A glyph placement of `source` under the device linear map `linear`
    /// at the device `origin`.
    ///
    /// The second of `glyph` names the source's edges once a placement of
    /// it has found them, so that its other placements need not look them
    /// up again.
    ///
    /// # Errors
    ///
    /// As `GlyphEdges::new` and `Shape::glyph_with_clip`.
    pub(crate) fn glyph(
        &mut self,
        (source, handle): (&GlyphSource, &mut Option<u32>),
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let kept = handle.and_then(|i| self.handles.get(u32_to_usize(i)));
        let edges = if let Some(edges) = kept {
            Arc::clone(edges)
        } else {
            let edges = self.edges(source, linear)?;
            *handle = u32::try_from(self.handles.len()).ok();
            self.handles.push(Arc::clone(&edges));
            edges
        };
        let shape = Shape::glyph_with_clip(&edges, origin, self.current_clip(), options)?;
        self.shape(shape)
    }

    /// The edges of `source` under `linear`, from this preparation or the
    /// store. They are a value of the store: they cost the render nothing,
    /// found or built (ADR 0016).
    fn edges(&mut self, source: &GlyphSource, linear: [f64; 4]) -> Result<Arc<GlyphEdges>> {
        let key = EdgesKey::new(source, linear);
        if let Some(edges) = self.edges.get(&key) {
            return Ok(Arc::clone(edges));
        }
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
        Ok(edges)
    }

    /// The hull of `source`'s outline under the device linear map
    /// `transform` relative to a placement's origin, as [`bounds::path`]
    /// gives it. Edges found in this preparation or kept in the store from
    /// an earlier one keep the hull; their handle is set in the second of
    /// `glyph`. Edges are not built before a placement meets the region.
    pub(crate) fn glyph_hull(
        &mut self,
        (source, handle): (&GlyphSource, &mut Option<u32>),
        transform: Transform,
    ) -> Option<Hull> {
        if let Some(edges) = handle.and_then(|i| self.handles.get(u32_to_usize(i))) {
            return edges.hull();
        }
        let key = EdgesKey::new(source, [transform.a, transform.b, transform.c, transform.d]);
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

    /// A clip state: the intersection of `paths`, outermost first.
    ///
    /// # Errors
    ///
    /// As `ClipShape::new`.
    pub(crate) fn clip(
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

    /// The prepared shapes and clips.
    pub(crate) fn finish(self) -> Shapes {
        Shapes {
            shapes: self.shapes,
            clips: self.clips,
        }
    }
}

/// The prepared shapes and clips of a scene, rasterized band by band in the
/// scratch storage of the caller's rasterizer.
#[derive(Debug, Default)]
pub(crate) struct Shapes {
    shapes: Vec<Shape>,
    clips: Vec<ClipShape>,
}

impl Shapes {
    /// Writes the coverage of `cover` in `band` to `out`, replacing it.
    pub(crate) fn rasterize(
        &self,
        rasterizer: &mut BandRasterizer,
        cover: Cover,
        band: u32,
        out: &mut BandCoverage,
    ) {
        let clip = cover.clip.and_then(|i| self.clips.get(u32_to_usize(i)));
        let shape = match cover.shape.map(|key| self.shapes.get(u32_to_usize(key))) {
            Some(Some(shape)) => Some(shape),
            // A key no preparation gave out covers nothing.
            Some(None) => {
                out.clear();
                return;
            }
            None => None,
        };
        rasterizer.rasterize(MarkCoverage { shape, clip }, band, out);
    }

    /// Writes the coverage within `band` of the leading `shapes`, which
    /// share no pixel and are all within `clip`, to `out`, replacing it:
    /// each pixel's coverage as [`Self::rasterize`] gives it for the one
    /// shape that can cover it, with zero coverage in the gaps of a row.
    /// Returns how many shapes it took, at least one when there are any.
    pub(crate) fn rasterize_run(
        &self,
        rasterizer: &mut BandRasterizer,
        shapes: &[u32],
        clip: Option<u32>,
        band: u32,
        out: &mut BandCoverage,
    ) -> usize {
        let mut shapes = shapes
            .iter()
            .take(RUN_SHAPES)
            .map_while(|&key| self.shapes.get(u32_to_usize(key)));
        let Some(first) = shapes.next() else {
            out.clear();
            return 0;
        };
        // The run has a fixed preparation bound; gathering its references
        // must not allocate inside the band executor.
        let mut run = [first; RUN_SHAPES];
        let mut count = 1;
        for shape in shapes {
            run[count] = shape;
            count += 1;
        }
        let clip = clip.and_then(|i| self.clips.get(u32_to_usize(i)));
        rasterizer.rasterize_run(&run[..count], clip, band, out)
    }

    /// The storage of its shapes, for the next preparation.
    pub(crate) fn into_storage(self) -> Storage {
        self.shapes
    }
}

/// The most shapes one call of [`Shapes::rasterize_run`] passes on.
const RUN_SHAPES: usize = 256;
