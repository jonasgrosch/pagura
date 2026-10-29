//! Coverage sources (ADR 0034 §4): the shapes and clips preparation builds
//! with the band rasterizer, and their coverage band by band.

use crate::plan::Cover;
use crate::prepare::bounds::{self, Hull};
use pagura_core::convert::u32_to_usize;
use pagura_core::hash::FastMap;
use pagura_core::{Built, Limits, Meter, Result, Store, StoreKey};
use pagura_interpret::{GlyphSource, OutlineKey, Pen};
use pagura_raster::{
    BandCoverage, BandRasterizer, ClipShape, DeviceRect, FillRule, GlyphEdges, MarkCoverage, Path,
    Point, RowOut, Shape, ShapeOptions, StrokeTemplates, Transform,
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
}

/// The shapes and clips of one preparation, with the glyph edges its
/// placements share.
#[derive(Debug)]
pub(crate) struct ShapeBuilder<'a> {
    store: &'a Store,
    limits: &'a Limits,
    shapes: Vec<Shape>,
    clips: Vec<ClipShape>,
    edges: FastMap<EdgesKey, u32>,
    /// Edges by the handles [`Self::glyph`] gives out.
    handles: Vec<Arc<GlyphEdges>>,
    /// The clip state shapes are prepared in.
    clip: Option<u32>,
    /// The strokes prepared so far, for their repeats elsewhere.
    strokes: StrokeTemplates,
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
    /// Prepared shapes, with glyph edge values cached in `store`.
    pub(crate) fn new(store: &'a Store, limits: &'a Limits) -> Self {
        Self {
            store,
            limits,
            shapes: Vec::new(),
            clips: Vec::new(),
            edges: FastMap::default(),
            handles: Vec::new(),
            clip: None,
            strokes: StrokeTemplates::default(),
        }
    }

    /// Prepares the next shapes within clip `clip` (`None`: unclipped).
    pub(crate) fn set_clip(&mut self, clip: Option<u32>) {
        self.clip = clip;
    }

    fn current_clip(&self) -> Option<&ClipShape> {
        self.clip.and_then(|k| self.clips.get(u32_to_usize(k)))
    }

    fn shape(&mut self, shape: Option<Shape>, bytes: &mut Meter) -> Result<Option<Prepared>> {
        let Some(shape) = shape else {
            return Ok(None);
        };
        let key = index(self.shapes.len())?;
        let prepared = Prepared {
            key,
            bounds: shape.bounds(),
            work: shape.work(),
        };
        // The payload is already live; keep it charged while the old and
        // replacement inline-slot buffers overlap during growth.
        let payload = shape.retained_bytes().saturating_sub(size_of::<Shape>());
        bytes.charge(payload)?;
        if let Err(error) = bytes.reserve_capacity(&mut self.shapes, 1) {
            drop(shape);
            bytes.refund(payload);
            return Err(error);
        }
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
        bytes: &mut Meter,
    ) -> Result<Option<Prepared>> {
        let shape = Shape::fill_with_clip(path, transform, rule, self.current_clip(), options)?;
        self.shape(shape, bytes)
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
        bytes: &mut Meter,
    ) -> Result<Option<Prepared>> {
        let clip = self.clip.and_then(|k| self.clips.get(u32_to_usize(k)));
        let shape = Shape::stroke_repeated(
            path,
            &pen.stroke,
            (inverse, transform),
            pen.adjustment,
            clip,
            options,
            &mut self.strokes,
        )?;
        self.shape(shape, bytes)
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
        bytes: &mut Meter,
    ) -> Result<Option<Prepared>> {
        let key = match handle.filter(|&i| self.handles.get(u32_to_usize(i)).is_some()) {
            Some(key) => key,
            None => self.edges(source, linear, bytes)?,
        };
        *handle = Some(key);
        let edges =
            self.handles
                .get(u32_to_usize(key))
                .ok_or(pagura_core::Error::invalid_argument(
                    "prepared glyph handle out of range",
                ))?;
        // Handle/table reservation precedes Store lookup on both a hit and
        // a build. Geometry sees the allowance left after those reservations.
        let limits = Limits {
            render_bytes: bytes.remaining(),
            ..*options.limits
        };
        let options = ShapeOptions {
            area: options.area,
            limits: &limits,
        };
        let shape = Shape::glyph_with_clip(edges, origin, self.current_clip(), &options)?;
        self.shape(shape, bytes)
    }

    /// The edges of `source` under `linear`, from this preparation or the
    /// store. They are a value of the store: they cost the render nothing,
    /// found or built (ADR 0016).
    fn edges(&mut self, source: &GlyphSource, linear: [f64; 4], bytes: &mut Meter) -> Result<u32> {
        let key = EdgesKey::new(source, linear);
        if let Some(&handle) = self.edges.get(&key) {
            return Ok(handle);
        }
        let handle = index(self.handles.len())?;
        bytes.reserve_map_capacity(&mut self.edges, 1)?;
        bytes.reserve_capacity(&mut self.handles, 1)?;
        let limits = self.limits;
        let edges = self.store.get_or_build(key, || {
            let edges = GlyphEdges::new(&source.outline, linear, limits)?;
            let bytes = edges.retained_bytes();
            Ok(Built {
                value: edges,
                bytes,
            })
        })?;
        self.edges.insert(key, handle);
        self.handles.push(edges);
        Ok(handle)
    }

    /// The hull of `source`'s outline under the device linear map
    /// `transform` relative to a placement's origin, as [`bounds::path`]
    /// gives it. Edges found in this preparation or kept in the store from
    /// an earlier one keep the hull; their handle is set in the second of
    /// `glyph`. Edges are not built before a placement meets the region.
    pub(crate) fn glyph_hull(
        &self,
        (source, handle): (&GlyphSource, &mut Option<u32>),
        transform: Transform,
    ) -> Option<Hull> {
        if let Some(edges) = handle.and_then(|i| self.handles.get(u32_to_usize(i))) {
            return edges.hull();
        }
        let key = EdgesKey::new(source, [transform.a, transform.b, transform.c, transform.d]);
        if let Some(&key) = self.edges.get(&key)
            && let Some(edges) = self.handles.get(u32_to_usize(key))
        {
            *handle = Some(key);
            return edges.hull();
        }
        // Looking up bounds must not allocate render-owned metadata only
        // on a warm Store, including for placements later culled.
        self.store.get(&key).map_or_else(
            || bounds::path(&source.outline, transform),
            |edges| edges.hull(),
        )
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
        bytes: &mut Meter,
    ) -> Result<Prepared> {
        // ClipShape's retained payload counts Inner and its geometry; its
        // shared allocation also owns two Arc reference counters.
        bytes.charge_items::<usize>(2)?;
        let limits = Limits {
            render_bytes: bytes.remaining(),
            ..*options.limits
        };
        let options = ShapeOptions {
            area: options.area,
            limits: &limits,
        };
        let clip = match ClipShape::new(paths, transform, &options) {
            Ok(clip) => clip,
            Err(error) => {
                bytes.refund(2 * size_of::<usize>());
                return Err(error);
            }
        };
        let key = index(self.clips.len())?;
        let prepared = Prepared {
            key,
            bounds: clip.bounds(),
            work: 0,
        };
        bytes.charge(clip.retained_bytes())?;
        if let Err(error) = bytes.reserve_capacity(&mut self.clips, 1) {
            let released = clip.retained_bytes() + 2 * size_of::<usize>();
            drop(clip);
            bytes.refund(released);
            return Err(error);
        }
        self.clips.push(clip);
        Ok(prepared)
    }

    /// The prepared shapes and clips.
    pub(crate) fn finish(self, bytes: &mut Meter) -> Result<Shapes> {
        let released = bytes
            .map_bytes(&self.edges)?
            .saturating_add(self.handles.capacity() * size_of::<Arc<GlyphEdges>>());
        let Self {
            shapes,
            clips,
            edges,
            handles,
            ..
        } = self;
        drop((edges, handles));
        bytes.refund(released);
        Ok(Shapes { shapes, clips })
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
    /// Owned containers and payloads; shared Store glyph values are excluded.
    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.shapes.capacity() * size_of::<Shape>()
            + self
                .shapes
                .iter()
                .map(|shape| shape.retained_bytes() - size_of::<Shape>())
                .sum::<usize>()
            + self.clips.capacity() * size_of::<ClipShape>()
            + self
                .clips
                .iter()
                .map(|clip| clip.retained_bytes() + 2 * size_of::<usize>())
                .sum::<usize>()
    }

    /// Writes the coverage of `cover` in `band` to `out`, replacing it.
    pub(crate) fn rasterize(
        &self,
        rasterizer: &mut BandRasterizer,
        cover: Cover,
        band: u32,
        out: &mut BandCoverage,
    ) {
        out.clear();
        self.rasterize_into(rasterizer, cover, band, out);
        out.finish();
    }

    /// Writes the coverage of `cover` in `band` to `out` row by row, each
    /// pixel once.
    pub(crate) fn rasterize_into(
        &self,
        rasterizer: &mut BandRasterizer,
        cover: Cover,
        band: u32,
        out: &mut dyn RowOut,
    ) {
        let clip = cover.clip.and_then(|i| self.clips.get(u32_to_usize(i)));
        let shape = match cover.shape.map(|key| self.shapes.get(u32_to_usize(key))) {
            Some(Some(shape)) => Some(shape),
            // A key no preparation gave out covers nothing.
            Some(None) => return,
            None => None,
        };
        rasterizer.rasterize_into(MarkCoverage { shape, clip }, band, out);
    }

    /// Writes the coverage of glyph placement `shape` in `band` to `out`,
    /// replacing it, as [`Self::rasterize`] does for it alone; its key names
    /// it across the bands of a render.
    pub(crate) fn rasterize_glyph(
        &self,
        rasterizer: &mut BandRasterizer,
        shape: u32,
        band: u32,
        out: &mut BandCoverage,
    ) {
        match self.shapes.get(u32_to_usize(shape)) {
            Some(glyph) => rasterizer.rasterize_glyph((shape, glyph), band, out),
            // A key no preparation gave out covers nothing.
            None => out.clear(),
        }
    }

    /// A keyed glyph's rows emitted directly, with the same reuse across
    /// bands as [`Self::rasterize_glyph`].
    pub(crate) fn rasterize_glyph_into(
        &self,
        rasterizer: &mut BandRasterizer,
        shape: u32,
        band: u32,
        out: &mut dyn RowOut,
    ) {
        if let Some(glyph) = self.shapes.get(u32_to_usize(shape)) {
            rasterizer.rasterize_glyph_into((shape, glyph), band, out);
        }
    }

    /// Writes the coverage within `band` of the leading `shapes`, which
    /// share no pixel and are all within `clip`, to `out`, replacing it:
    /// each pixel's coverage as [`Self::rasterize`] gives it for the one
    /// shape that can cover it, with zero coverage in the gaps of a row.
    /// Each shape goes with its key, which names it across the bands of a
    /// render. Returns how many shapes it took, at least one when there are
    /// any.
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
            .map_while(|&key| Some((key, self.shapes.get(u32_to_usize(key))?)));
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
}

/// The most shapes one call of [`Shapes::rasterize_run`] passes on.
const RUN_SHAPES: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{ErrorKind, Rect};
    use pagura_raster::Area;

    fn rectangle(limits: &Limits) -> Shape {
        let path = Path::rect(Rect::from_corners(0.25, 0.25, 1.75, 1.75).unwrap());
        Shape::fill(
            &path,
            Transform::IDENTITY,
            FillRule::NonZero,
            &ShapeOptions {
                area: Area {
                    region: DeviceRect {
                        x0: 0,
                        y0: 0,
                        x1: 2,
                        y1: 2,
                    },
                    band_height: 1,
                },
                limits,
            },
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn shape_capacity_is_reserved_before_growth_beside_its_payload() {
        let limits = Limits::default();
        let store = Store::new(1 << 20);
        let shape = rectangle(&limits);
        let payload = shape.retained_bytes() - size_of::<Shape>();
        let slot = size_of::<Shape>();
        let mut builder = ShapeBuilder::new(&store, &limits);
        let mut tiny = Meter::new(payload + slot - 1, "shape memory");
        assert_eq!(
            builder.shape(Some(shape), &mut tiny).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!((builder.shapes.len(), builder.shapes.capacity()), (0, 0));
        assert_eq!(tiny.used(), 0);

        let mut bytes = Meter::new(2 * payload + 3 * slot - 1, "shape memory");
        builder.shape(Some(rectangle(&limits)), &mut bytes).unwrap();
        assert_eq!(
            (
                builder.shapes.len(),
                builder.shapes.capacity(),
                bytes.used()
            ),
            (1, 1, payload + slot)
        );
        let error = builder
            .shape(Some(rectangle(&limits)), &mut bytes)
            .unwrap_err();
        assert_eq!(error.message, "shape memory");
        assert_eq!((builder.shapes.len(), builder.shapes.capacity()), (1, 1));
        assert_eq!(bytes.used(), payload + slot);
    }

    #[test]
    fn finishing_drops_only_metadata_and_keeps_capacity_and_payload() {
        let limits = Limits::default();
        let store = Store::new(1 << 20);
        let mut builder = ShapeBuilder::new(&store, &limits);
        let source = GlyphSource {
            outline: Arc::new(Path::rect(Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap())),
            key: OutlineKey {
                font: pagura_interpret::FontKey(4),
                glyph: 3,
            },
            linear: Transform::IDENTITY,
        };
        let mut bytes = Meter::new(1 << 20, "shape memory");
        builder
            .edges(&source, [1.0, 0.0, 0.0, 1.0], &mut bytes)
            .unwrap();
        for _ in 0..3 {
            builder.shape(Some(rectangle(&limits)), &mut bytes).unwrap();
        }
        let before = bytes.used();
        let metadata = bytes.map_bytes(&builder.edges).unwrap()
            + builder.handles.capacity() * size_of::<Arc<GlyphEdges>>();
        let shapes = builder.finish(&mut bytes).unwrap();
        assert!(shapes.shapes.capacity() > shapes.shapes.len());
        assert_eq!(bytes.used(), before - metadata);
        assert_eq!(bytes.used(), shapes.retained_bytes());
    }

    #[test]
    fn warm_hull_lookup_is_read_only_and_handle_growth_precedes_store_build() {
        let limits = Limits::default();
        let store = Store::new(1 << 20);
        let source = GlyphSource {
            outline: Arc::new(Path::rect(Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap())),
            key: OutlineKey {
                font: pagura_interpret::FontKey(4),
                glyph: 3,
            },
            linear: Transform::IDENTITY,
        };
        let mut builder = ShapeBuilder::new(&store, &limits);
        let mut none = Meter::new(0, "shape memory");
        assert_eq!(
            builder
                .edges(&source, [1.0, 0.0, 0.0, 1.0], &mut none)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!(store.retained_bytes(), 0);
        let mut bytes = Meter::new(1 << 20, "shape memory");
        builder
            .edges(&source, [1.0, 0.0, 0.0, 1.0], &mut bytes)
            .unwrap();
        let fresh = ShapeBuilder::new(&store, &limits);
        let mut handle = None;
        assert_eq!(
            fresh.glyph_hull((&source, &mut handle), Transform::IDENTITY),
            Some([0.0, 0.0, 1.0, 1.0])
        );
        assert!(handle.is_none());
        assert!(fresh.edges.is_empty());
        assert_eq!(fresh.handles.capacity(), 0);
    }
}
