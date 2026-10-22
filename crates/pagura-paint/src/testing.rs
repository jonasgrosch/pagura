//! Test support: a scene builder, prepared rectangle shapes, and renders
//! through this crate and through the reference renderer.

use crate::prepare::{PreparedScene, ShapeBuilder, Shapes};
use crate::request::RenderRequest;
use pagura_color::{ColorSpace, Components};
use pagura_core::{Limits, Position, Rect, Result, Store};
use pagura_interpret::{
    ClipId, ClipNode, Color, Composite, FillNode, Group, GroupAttributes, GroupCause, GroupId,
    GroupRole, MarkInfo, MaskId, MaskKind, Node, NodeId, PageSpace, Paint, PaintId, Scene,
    SoftMask, StateId,
};
use pagura_raster::{Area, DeviceRect, FillRule, Path, Rgba8, ShapeOptions, Transform};
use std::sync::Arc;

/// A device rectangle `[x0, y0, x1, y1]` in f64.
pub(crate) type Box4 = [f64; 4];

/// The prepared shapes of `paths`, filled in device space over `area`,
/// keyed in order.
pub(crate) fn path_shapes(area: Area, paths: &[Path]) -> Shapes {
    let store = Store::new(1 << 20);
    let limits = Limits::default();
    let options = ShapeOptions {
        area,
        limits: &limits,
    };
    let mut builder = ShapeBuilder::new(&store, &limits, Vec::new());
    for (i, path) in paths.iter().enumerate() {
        let prepared = builder
            .fill(path, Transform::IDENTITY, FillRule::NonZero, &options)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("shape {i} covers no pixel"));
        assert_eq!(prepared.key, index(i));
    }
    builder.finish()
}

/// Builds scenes in device space (an identity request transform unless
/// `scale` is set).
pub(crate) struct Build {
    pub(crate) scene: Scene,
    /// Device pixels per unit.
    pub(crate) scale: f64,
    /// The background.
    pub(crate) background: Rgba8,
    /// Overprint simulation.
    pub(crate) simulate: bool,
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
            cause: GroupCause::Document,
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
            simulate: false,
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
    pub(crate) fn rect(
        &mut self,
        parent: GroupId,
        r: Box4,
        paint: PaintId,
        info: MarkInfo,
    ) -> NodeId {
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
    pub(crate) fn group(
        &mut self,
        role: GroupRole,
        attributes: GroupAttributes,
        info: MarkInfo,
    ) -> GroupId {
        self.scene.groups.push(Group {
            role,
            cause: GroupCause::Document,
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
        if self.simulate {
            request.overprint = crate::request::OverprintMode::Simulate;
        }
        request
    }

    /// Prepares `request` with a store of its own.
    pub(crate) fn prepare(
        &self,
        request: &RenderRequest,
        limits: &Limits,
    ) -> Result<PreparedScene> {
        crate::prepare(&self.scene, request, &Store::new(1 << 20), limits)
    }

    /// The reference's exact rendering of the whole page.
    pub(crate) fn reference(&self) -> pagura_reference::Rendering {
        let mut request = pagura_reference::Request::new(self.transform(), self.page());
        request.background = self.background;
        if self.simulate {
            request.overprint = pagura_reference::Overprint::Simulate;
        }
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
    for (i, (pixel, value)) in bytes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(exact.values())
        .enumerate()
    {
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

/// Renders the whole page and checks it against the reference: within one
/// level everywhere. Returns the bytes.
pub(crate) fn check(b: &Build) -> Vec<u8> {
    let bytes = b.render_page();
    assert_within_one_level(&bytes, &b.reference());
    bytes
}
