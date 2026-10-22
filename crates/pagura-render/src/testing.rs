//! Test doubles: scenes built by hand, as a recorder would record them, and
//! one-page documents written in memory.

use pagura_color::{ColorSpace, Colorant, Components, ExponentialFunction, Function, Separation};
use pagura_core::{Dictionary, Name, ObjectId, Position, Rect, Value};
use pagura_document::{Document, OpenOptions, Rotation};
use pagura_interpret::{
    ClipId, ClipNode, Color, Composite, FillNode, Group, GroupAttributes, GroupId, GroupRole,
    MarkInfo, Node, NodeId, PageSpace, Paint, PaintId, Pen, PenId, Scene, StateId, StrokeNode,
};
use pagura_raster::{Adjustment, FillRule, Path, Stroke, Transform};
use pagura_write::{Builder, NewObject};
use std::sync::Arc;

/// A rectangle from its corners.
pub(crate) fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::from_corners(x0, y0, x1, y1).unwrap()
}

/// A scene under construction: an empty page group with one opaque state.
pub(crate) struct SceneBuilder {
    pub(crate) scene: Scene,
}

impl SceneBuilder {
    /// An empty page with crop box `crop`.
    pub(crate) fn new(crop: Rect) -> Self {
        let root = Group {
            role: GroupRole::Page,
            attributes: GroupAttributes::default(),
            bbox: None,
            view: None,
            children: Vec::new(),
            mark: mark(None),
        };
        Self {
            scene: Scene {
                page: PageSpace {
                    crop,
                    rotation: Rotation::None,
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
        }
    }

    /// A solid paint in DeviceRGB.
    pub(crate) fn rgb(&mut self) -> PaintId {
        self.paint(ColorSpace::DeviceRgb, &[0.0, 0.0, 1.0])
    }

    /// A solid paint in a Separation `None` space, which paints nothing.
    pub(crate) fn invisible(&mut self) -> PaintId {
        let tint = Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.0],
            c1: vec![1.0],
            exponent: 1.0,
        })
        .unwrap();
        let space = ColorSpace::Separation(Arc::new(Separation {
            colorant: Colorant::None,
            alternate: Arc::new(ColorSpace::DeviceGray),
            tint: Arc::new(tint),
        }));
        self.paint(space, &[1.0])
    }

    fn paint(&mut self, space: ColorSpace, components: &[f64]) -> PaintId {
        self.scene.paints.push(Arc::new(Paint::Solid(Color {
            space: Arc::new(space),
            components: Components::from_slice(components),
        })));
        PaintId(index(self.scene.paints.len() - 1))
    }

    /// A pen of `width` under `transform`.
    pub(crate) fn pen(&mut self, width: f64, transform: Transform) -> PenId {
        self.scene.pens.push(Arc::new(Pen {
            stroke: Stroke {
                width,
                ..Stroke::default()
            },
            transform,
            adjustment: Adjustment::None,
        }));
        PenId(index(self.scene.pens.len() - 1))
    }

    /// A clip by `area`, inside `parent`.
    pub(crate) fn clip(&mut self, parent: Option<ClipId>, area: Rect) -> ClipId {
        self.scene.clips.push(ClipNode {
            parent,
            path: Arc::new(Path::rect(area)),
            rule: FillRule::NonZero,
        });
        ClipId(index(self.scene.clips.len() - 1))
    }

    /// Appends `node` to `parent`.
    pub(crate) fn node(&mut self, parent: GroupId, node: Node) -> NodeId {
        self.scene.nodes.push(node);
        let id = NodeId(index(self.scene.nodes.len() - 1));
        self.scene.groups[parent.index()].children.push(id);
        id
    }

    /// A fill of `path` with `paint` under `clip`.
    pub(crate) fn fill(
        &mut self,
        parent: GroupId,
        path: Path,
        paint: PaintId,
        clip: Option<ClipId>,
    ) {
        self.node(
            parent,
            Node::Fill(FillNode {
                path: Arc::new(path),
                rule: FillRule::NonZero,
                paint,
                mark: mark(clip),
            }),
        );
    }

    /// A stroke of `path` with `pen` and `paint`.
    pub(crate) fn stroke(&mut self, parent: GroupId, path: Path, pen: PenId, paint: PaintId) {
        self.node(
            parent,
            Node::Stroke(StrokeNode {
                path: Arc::new(path),
                pen,
                paint,
                mark: mark(None),
            }),
        );
    }

    /// A transparency group inside `parent` with an optional bounding box.
    pub(crate) fn group(&mut self, parent: GroupId, bbox: Option<Rect>) -> GroupId {
        self.scene.groups.push(Group {
            role: GroupRole::Transparency,
            attributes: GroupAttributes::default(),
            bbox: bbox.map(|bbox| Arc::new(Path::rect(bbox))),
            view: None,
            children: Vec::new(),
            mark: mark(None),
        });
        let id = GroupId(index(self.scene.groups.len() - 1));
        self.node(parent, Node::Group(id));
        id
    }
}

fn mark(clip: Option<ClipId>) -> MarkInfo {
    MarkInfo {
        state: StateId(0),
        clip,
        position: Position::Unknown,
    }
}

fn index(n: usize) -> u32 {
    u32::try_from(n).unwrap()
}

/// A one-page document whose page dictionary has `entries` besides its
/// type, parent and empty content.
pub(crate) fn document(entries: &[(&[u8], Value)]) -> Document {
    let id = |number| ObjectId::new(number, 0);
    let dictionary = |entries: Vec<(&[u8], Value)>| {
        Value::Dictionary(
            entries
                .into_iter()
                .map(|(key, value)| (Name::new(key), value))
                .collect(),
        )
    };
    let mut builder = Builder::default();
    let catalog = dictionary(vec![
        (b"Type", Value::name(b"Catalog")),
        (b"Pages", Value::Reference(id(2))),
    ]);
    builder.insert(id(1), NewObject::Value(catalog)).unwrap();
    let pages = dictionary(vec![
        (b"Type", Value::name(b"Pages")),
        (b"Kids", Value::Array(vec![Value::Reference(id(3))])),
        (b"Count", Value::Integer(1)),
    ]);
    builder.insert(id(2), NewObject::Value(pages)).unwrap();
    let mut page = vec![
        (&b"Type"[..], Value::name(b"Page")),
        (b"Parent", Value::Reference(id(2))),
        (b"Contents", Value::Reference(id(4))),
    ];
    page.extend(entries.iter().cloned());
    builder
        .insert(id(3), NewObject::Value(dictionary(page)))
        .unwrap();
    let content = NewObject::Stream {
        dictionary: Dictionary::new(),
        data: Vec::new(),
    };
    builder.insert(id(4), content).unwrap();
    Document::open(builder.finish(id(1)).unwrap(), &OpenOptions::default()).unwrap()
}

/// A `MediaBox` entry.
pub(crate) fn media_box(width: i64, height: i64) -> (&'static [u8], Value) {
    let corners = [0, 0, width, height].map(Value::Integer);
    (b"MediaBox", Value::Array(corners.to_vec()))
}
