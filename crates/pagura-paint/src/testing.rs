//! Test doubles: a backend of axis-aligned rectangles with exact coverage
//! and device colours (standing in for the band rasterizer and the paint
//! sources), a scene builder, and renders through this crate and through
//! the reference renderer.

use crate::paint::{Convert, PaintSpans};
use crate::prepare::{Backend, PaintRequest, Prepared, PreparedScene, Producer, prepare_with};
use crate::request::RenderRequest;
use crate::source::Source;
use pagura_color::{ColorSpace, Components, Conversion, Painted, Process, Target};
use pagura_core::{Error, Limits, Meter, Position, Rect, Result, Store};
use pagura_interpret::{
    ClipId, ClipNode, Color, Composite, FillNode, GlyphSource, Group, GroupAttributes, GroupId,
    GroupRole, Image, MarkInfo, MaskId, MaskKind, Node, NodeId, PageSpace, Paint, PaintId, Pen,
    Scene, Shading, SoftMask, StateId,
};
use pagura_raster::{
    Area, BandCoverage, DeviceRect, FillRule, Path, Pixmap, Point, Rgba8, Segment, ShapeOptions,
    Transform, quantize_unit,
};
use std::sync::Arc;

/// A device rectangle `[x0, y0, x1, y1]` in f64.
pub(crate) type Box4 = [f64; 4];

/// The backend: every path must be an axis-aligned rectangle in device
/// space; colours are device colours of the target's own family.
#[derive(Debug, Default)]
pub(crate) struct Rects {
    shapes: Vec<Box4>,
    clips: Vec<Box4>,
}

fn rectangle(path: &Path, transform: Transform) -> Result<Option<Box4>> {
    let mut points = Vec::new();
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => points.push(transform.apply(p)),
            Segment::Close => {}
            Segment::CubicTo { .. } => {
                return Err(Error::syntax("the test backend takes rectangles only"));
            }
        }
    }
    let Some(hull) = crate::prepare::bounds::points(points.iter().copied()) else {
        return Ok(None);
    };
    let corner = |p: &Point| (p.x == hull[0] || p.x == hull[2]) && (p.y == hull[1] || p.y == hull[3]);
    if !points.iter().all(corner) {
        return Err(Error::syntax("the test backend takes rectangles only"));
    }
    Ok(Some(hull))
}

fn meets(rect: Box4, area: Area) -> Option<DeviceRect> {
    let r = area.region;
    let bounds = crate::prepare::bounds::device(Some(rect), r);
    (!bounds.is_empty() && rect[2] > rect[0] && rect[3] > rect[1]).then_some(bounds)
}

impl Rects {
    fn shape(&mut self, rect: Option<Box4>, options: &ShapeOptions<'_>) -> Option<Prepared> {
        let rect = rect?;
        let bounds = meets(rect, options.area)?;
        let key = u32::try_from(self.shapes.len()).ok()?;
        self.shapes.push(rect);
        Some(Prepared {
            key,
            bounds,
            work: crate::plan::area(bounds) + 1,
            bytes: size_of::<Box4>(),
        })
    }
}

impl Backend for Rects {
    fn fill(
        &mut self,
        path: &Path,
        transform: Transform,
        _rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let rect = rectangle(path, transform)?;
        Ok(self.shape(rect, options))
    }

    fn stroke(
        &mut self,
        path: &Path,
        (pen, _): (&Pen, Transform),
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let points = path.segments().iter().filter_map(|s| match *s {
            Segment::MoveTo(p) | Segment::LineTo(p) => Some(transform.apply(p)),
            _ => None,
        });
        let half = pen.stroke.width / 2.0 * transform.a.abs();
        let rect = crate::prepare::bounds::points(points)
            .map(|h| [h[0] - half, h[1] - half, h[2] + half, h[3] + half]);
        Ok(self.shape(rect, options))
    }

    fn glyph(
        &mut self,
        source: &GlyphSource,
        linear: [f64; 4],
        origin: Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Prepared>> {
        let [a, b, c, d] = linear;
        let transform = Transform::new(a, b, c, d, origin.x, origin.y);
        let rect = rectangle(&source.outline, transform)?;
        Ok(self.shape(rect, options))
    }

    fn clip(
        &mut self,
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Prepared> {
        let mut rect = [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY];
        for (path, _) in paths {
            let r = rectangle(path, transform)?.unwrap_or([0.0; 4]);
            rect = [rect[0].max(r[0]), rect[1].max(r[1]), rect[2].min(r[2]), rect[3].min(r[3])];
        }
        let key = u32::try_from(self.clips.len()).unwrap_or(u32::MAX);
        self.clips.push(rect);
        Ok(Prepared {
            key,
            bounds: meets(rect, options.area).unwrap_or_default(),
            work: 0,
            bytes: size_of::<Box4>(),
        })
    }

    fn paint(
        &mut self,
        paint: &Paint,
        request: &PaintRequest<'_>,
        functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>> {
        let Paint::Solid(color) = paint else {
            return Err(Error::syntax("the test backend paints solid colours only"));
        };
        functions.charge(1)?;
        Ok(Box::new(native(color, request.context.target)?))
    }

    fn shading(
        &mut self,
        _shading: &Shading,
        _to_device: Transform,
        _request: &PaintRequest<'_>,
        _functions: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>> {
        Err(Error::syntax("the test backend has no shadings"))
    }

    fn image(
        &mut self,
        _image: &Image,
        _to_device: Transform,
        _stencil: Option<Box<dyn PaintSpans>>,
        _request: &PaintRequest<'_>,
        _work: &mut Meter,
    ) -> Result<Box<dyn PaintSpans>> {
        Err(Error::syntax("the test backend has no images"))
    }

    fn converter(
        &mut self,
        _space: &ColorSpace,
        _target: &Target,
        _conversion: &Conversion,
    ) -> Result<Box<dyn Convert>> {
        Err(Error::syntax("the test backend has no converters"))
    }

    fn finish(self, area: Area, _bytes: &mut Meter) -> Result<Box<dyn Producer>> {
        Ok(Box::new(RectCoverage {
            area,
            shapes: self.shapes,
            clips: self.clips,
        }))
    }
}

/// A device colour in a target of its own family (10.4 defines the
/// identity), as the reference converts it: spot channels unpainted.
pub(crate) fn native(color: &Color, target: &Target) -> Result<Source> {
    let process = match *color.space {
        ColorSpace::DeviceGray => Process::Gray,
        ColorSpace::DeviceRgb => Process::Rgb,
        ColorSpace::DeviceCmyk => Process::Cmyk,
        _ => return Err(Error::syntax("the test backend converts device colours only")),
    };
    if process != target.process {
        return Err(Error::syntax("the test backend converts within a family only"));
    }
    let mut values = vec![1.0; target.channels()];
    for (value, &c) in values.iter_mut().zip(color.components.iter()) {
        let c = c.clamp(0.0, 1.0);
        *value = if process == Process::Cmyk { 1.0 - c } else { c };
    }
    Ok(Source::Solid {
        color: values.into_boxed_slice(),
        painted: Painted((1 << process.components()) - 1),
    })
}

/// Exact coverage of rectangles, quantized once.
#[derive(Debug)]
struct RectCoverage {
    area: Area,
    shapes: Vec<Box4>,
    clips: Vec<Box4>,
}

fn overlap(pixel: u32, low: f64, high: f64) -> f64 {
    let p = f64::from(pixel);
    (high.min(p + 1.0) - low.max(p)).clamp(0.0, 1.0)
}

impl Producer for RectCoverage {
    fn rasterize(&mut self, shape: Option<u32>, clip: Option<u32>, band: u32, out: &mut BandCoverage) {
        out.clear();
        let region = self.area.region;
        let whole = [
            f64::from(region.x0),
            f64::from(region.y0),
            f64::from(region.x1),
            f64::from(region.y1),
        ];
        let get = |set: &[Box4], i: Option<u32>| {
            i.and_then(|i| set.get(pagura_core::convert::u32_to_usize(i)).copied())
        };
        let s = get(&self.shapes, shape).unwrap_or(whole);
        let c = get(&self.clips, clip).unwrap_or(whole);
        let r = [s[0].max(c[0]), s[1].max(c[1]), s[2].min(c[2]), s[3].min(c[3])];
        let mut cells = Vec::new();
        for y in self.area.band_rows(band) {
            let vertical = overlap(y, r[1], r[3]);
            if vertical <= 0.0 {
                continue;
            }
            out.push_row(y);
            let mut full: Option<(u32, u32)> = None;
            let mut partial: Option<u32> = None;
            for x in region.x0..=region.x1 {
                let area = if x < region.x1 {
                    overlap(x, r[0], r[2]) * vertical
                } else {
                    0.0
                };
                let byte = quantize_unit(area);
                let is_full = area >= 1.0;
                if !is_full && let Some((start, len)) = full.take() {
                    out.push_full(start, len);
                }
                if (is_full || byte == 0) && let Some(start) = partial.take() {
                    out.push_partial(start, &cells);
                    cells.clear();
                }
                if is_full {
                    full = Some(full.map_or((x, 1), |(start, len)| (start, len + 1)));
                } else if byte > 0 {
                    partial.get_or_insert(x);
                    cells.push(byte);
                }
            }
        }
        out.finish();
    }
}

/// Builds scenes in device space (an identity request transform unless
/// `scale` is set).
pub(crate) struct Build {
    pub(crate) scene: Scene,
    /// Device pixels per unit.
    pub(crate) scale: f64,
    /// The background.
    pub(crate) background: Rgba8,
}

/// Mark state without clip.
pub(crate) fn mark(state: StateId) -> MarkInfo {
    MarkInfo {
        state,
        clip: None,
        position: Position::Unknown,
    }
}

/// A rectangle path.
pub(crate) fn rect_path(r: Box4) -> Arc<Path> {
    Arc::new(Path::rect(
        Rect::from_corners(r[0], r[1], r[2], r[3]).unwrap_or_else(|| unreachable!()),
    ))
}

impl Build {
    /// An empty RGB page of `width` by `height` pixels.
    pub(crate) fn new(width: f64, height: f64) -> Self {
        let root = Group {
            role: GroupRole::Page,
            attributes: GroupAttributes::default(),
            bbox: None,
            view: None,
            children: Vec::new(),
            mark: mark(StateId(0)),
        };
        Self {
            scene: Scene {
                page: PageSpace {
                    crop: Rect::from_corners(0.0, 0.0, width, height)
                        .unwrap_or_else(|| unreachable!()),
                    rotation: pagura_document::Rotation::None,
                    user_unit: 1.0,
                },
                print_intent: false,
                root: GroupId(0),
                nodes: Vec::new(),
                groups: vec![root],
                masks: Vec::new(),
                tilings: Vec::new(),
                clips: Vec::new(),
                states: vec![Arc::new(Composite::default())],
                paints: Vec::new(),
                pens: Vec::new(),
                bytes: 0,
            },
            scale: 1.0,
            background: Rgba8::WHITE,
        }
    }

    /// The request transform.
    pub(crate) fn transform(&self) -> Transform {
        Transform::scale(self.scale, self.scale)
    }

    /// A composite state.
    pub(crate) fn state(&mut self, edit: impl FnOnce(&mut Composite)) -> StateId {
        let mut state = Composite::default();
        edit(&mut state);
        self.scene.states.push(Arc::new(state));
        StateId(index(self.scene.states.len() - 1))
    }

    /// A solid paint in `space`.
    pub(crate) fn color(&mut self, space: ColorSpace, components: &[f64]) -> PaintId {
        self.paint(Paint::Solid(Color {
            space: Arc::new(space),
            components: Components::from_slice(components),
        }))
    }

    /// A DeviceRGB paint.
    pub(crate) fn rgb(&mut self, rgb: [f64; 3]) -> PaintId {
        self.color(ColorSpace::DeviceRgb, &rgb)
    }

    /// A paint.
    pub(crate) fn paint(&mut self, paint: Paint) -> PaintId {
        self.scene.paints.push(Arc::new(paint));
        PaintId(index(self.scene.paints.len() - 1))
    }

    /// Appends `node` to `parent`.
    pub(crate) fn node(&mut self, parent: GroupId, node: Node) -> NodeId {
        self.scene.nodes.push(node);
        let id = NodeId(index(self.scene.nodes.len() - 1));
        if let Some(group) = self.scene.groups.get_mut(parent.index()) {
            group.children.push(id);
        }
        id
    }

    /// A rectangle fill.
    pub(crate) fn rect(&mut self, parent: GroupId, r: Box4, paint: PaintId, info: MarkInfo) -> NodeId {
        self.node(
            parent,
            Node::Fill(FillNode {
                path: rect_path(r),
                rule: FillRule::NonZero,
                paint,
                mark: info,
            }),
        )
    }

    /// A group of `role`, not yet painted anywhere.
    pub(crate) fn group(&mut self, role: GroupRole, attributes: GroupAttributes, info: MarkInfo) -> GroupId {
        self.scene.groups.push(Group {
            role,
            attributes,
            bbox: None,
            view: None,
            children: Vec::new(),
            mark: info,
        });
        GroupId(index(self.scene.groups.len() - 1))
    }

    /// A transparency group painted in `parent`.
    pub(crate) fn transparency(
        &mut self,
        parent: GroupId,
        attributes: GroupAttributes,
        info: MarkInfo,
    ) -> GroupId {
        let group = self.group(GroupRole::Transparency, attributes, info);
        self.node(parent, Node::Group(group));
        group
    }

    /// A soft mask of `kind` whose group is returned with it.
    pub(crate) fn mask(
        &mut self,
        kind: MaskKind,
        attributes: GroupAttributes,
        backdrop: Option<&[f64]>,
    ) -> (MaskId, GroupId) {
        let group = self.group(GroupRole::SoftMask { kind }, attributes, mark(StateId(0)));
        self.scene.masks.push(SoftMask {
            group,
            kind,
            backdrop: backdrop.map(Components::from_slice),
            transfer: None,
            position: Position::Unknown,
        });
        (MaskId(index(self.scene.masks.len() - 1)), group)
    }

    /// A clip of `r` inside `parent`.
    pub(crate) fn clip(&mut self, parent: Option<ClipId>, r: Box4) -> ClipId {
        self.scene.clips.push(ClipNode {
            parent,
            path: rect_path(r),
            rule: FillRule::NonZero,
        });
        ClipId(index(self.scene.clips.len() - 1))
    }

    /// The page in device pixels.
    pub(crate) fn page(&self) -> DeviceRect {
        let (width, height) = self.scene.page.device_size(self.scale).unwrap_or((1, 1));
        DeviceRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        }
    }

    /// A request for `region` with `band_height`.
    pub(crate) fn request(&self, region: DeviceRect, band_height: u32) -> RenderRequest {
        let mut request = RenderRequest::new(self.transform(), self.page(), region);
        request.band_height = band_height;
        request.background = self.background;
        request
    }

    /// Prepares `request` with the rectangle backend.
    pub(crate) fn prepare(&self, request: &RenderRequest, limits: &Limits) -> Result<PreparedScene> {
        let store = Store::new(1 << 20);
        prepare_with(&self.scene, request, (&store, limits), Rects::default())
    }

    /// Renders `request` into region-sized premultiplied RGBA8, checking
    /// that preparation reserved at least what rendering used.
    pub(crate) fn render(&self, request: &RenderRequest) -> Result<Vec<u8>> {
        Ok(self.render_charged(request)?.0)
    }

    /// [`Self::render`] with the reserved charges.
    pub(crate) fn render_charged(&self, request: &RenderRequest) -> Result<(Vec<u8>, crate::Charges)> {
        let mut prepared = self.prepare(request, &Limits::default())?;
        let region = request.region;
        let mut target = Pixmap::new(region.width(), region.height(), Rgba8::TRANSPARENT, usize::MAX)?;
        prepared.render(&mut target);
        let (used, reserved) = (prepared.usage(), prepared.charges());
        assert!(
            used.render_bytes() <= reserved.render_bytes()
                && used.render_work() <= reserved.render_work()
                && used.function_work() <= reserved.function_work(),
            "used {used:?} beyond reserved {reserved:?}"
        );
        Ok((target.data().to_vec(), reserved))
    }

    /// Renders the whole page.
    pub(crate) fn render_page(&self) -> Vec<u8> {
        self.render(&self.request(self.page(), RenderRequest::BAND_HEIGHT))
            .unwrap_or_else(|e| panic!("render failed: {e}"))
    }

    /// The reference's exact rendering of the whole page.
    pub(crate) fn reference(&self) -> pagura_reference::Rendering {
        let mut request = pagura_reference::Request::new(self.transform(), self.page());
        request.background = self.background;
        pagura_reference::render(&self.scene, &request, &Limits::default())
            .unwrap_or_else(|e| panic!("reference failed: {e}"))
    }
}

/// `n` as an id.
pub(crate) fn index(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Asserts that every channel of `bytes` is within one level of the exact
/// value, and returns the largest difference in levels.
pub(crate) fn assert_within_one_level(bytes: &[u8], exact: &pagura_reference::Rendering) -> f64 {
    let mut worst: f64 = 0.0;
    for (i, (pixel, value)) in bytes.chunks_exact(4).zip(exact.values()).enumerate() {
        for (channel, (&byte, &v)) in pixel.iter().zip(value).enumerate() {
            let difference = (f64::from(byte) - v * 255.0).abs();
            worst = worst.max(difference);
            assert!(
                difference <= 1.0,
                "pixel {i} channel {channel}: {byte} against exact {}",
                v * 255.0
            );
        }
    }
    worst
}

/// Asserts that `bytes` equal the reference's correctly rounded output
/// except where the exact value is within `tie` levels of a half level.
pub(crate) fn assert_rounded(bytes: &[u8], exact: &pagura_reference::Rendering) {
    let rounded = exact.to_rgba8();
    for (i, ((&byte, &expected), v)) in bytes
        .iter()
        .zip(&rounded)
        .zip(exact.values().iter().flatten())
        .enumerate()
    {
        let level = v * 255.0;
        let near_tie = (level - level.floor() - 0.5).abs() < 1e-6;
        assert!(
            byte == expected || near_tie,
            "channel {i}: {byte} against correctly rounded {expected} ({level})"
        );
    }
}

/// Renders the whole page and checks it against the reference: within one
/// level everywhere. Returns the bytes.
pub(crate) fn check(b: &Build) -> Vec<u8> {
    let bytes = b.render_page();
    assert_within_one_level(&bytes, &b.reference());
    bytes
}
