//! Glyph runs into the scene: filled glyphs as placements of shared
//! sources (ADR 0013), stroked glyphs as strokes of their outlines.

use super::SceneRecorder;
use crate::device::{Glyph, GlyphRun, OutlineKey};
use crate::hash::FastMap;
use crate::model::{Composite, FillStroke, GroupAttributes, GroupCause};
use crate::scene::{
    GlyphSource, GlyphsNode, Group, GroupRole, MarkInfo, Node, NodeId, PaintId, Placement, StateId,
};
use pagura_core::{Error, Result};
use pagura_raster::{FillRule, Path, Point, Segment, Transform};
use std::sync::Arc;

/// The placement and source tables one glyphs node keeps at most.
const MAX_PLACEMENTS: usize = 65_535;

/// The identity of a glyph source: outline and linear map, signed zeros
/// distinguished.
type SourceKey = (OutlineKey, [u64; 4]);

/// The glyph state of a recording.
#[derive(Debug, Default)]
pub(super) struct Glyphs {
    /// The glyphs node later runs may extend.
    open: Option<NodeId>,
    /// The sources of the open node, by identity.
    sources: FastMap<SourceKey, u32>,
    /// The hulls of the sources met: the outline's under the linear map.
    hulls: FastMap<SourceKey, [Point; 2]>,
    /// The fill-and-stroke group states derived for interned state pairs.
    derived: FastMap<(StateId, StateId), FillStroke>,
}

/// What the filled glyphs of one run share: their paint and mark, and the
/// glyphs node they extend.
#[derive(Debug, Clone, Copy)]
struct Target {
    paint: PaintId,
    mark: MarkInfo,
    node: NodeId,
}

impl SceneRecorder {
    pub(super) fn record_glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        let fills = run.mode.fills();
        let strokes = run.mode.strokes();
        let mut target = None;
        for glyph in run.glyphs {
            let Some(outline) = &glyph.outline else {
                continue;
            };
            match (fills, strokes) {
                (true, false) => self.place(run, glyph, outline, &mut target)?,
                (false, true) => self.stroke_glyph(run, glyph, outline, None)?,
                (true, true) => self.fill_stroke_glyph(run, glyph, outline)?,
                (false, false) => {}
            }
        }
        Ok(())
    }

    /// Adds a filled glyph to the open glyphs node, or to a new one. The
    /// run's first placed glyph interns its paint and mark into `target`.
    fn place(
        &mut self,
        run: &GlyphRun<'_>,
        glyph: &Glyph,
        outline: &Arc<Path>,
        target: &mut Option<Target>,
    ) -> Result<()> {
        let node = self.target(run, target)?.node;
        let linear = Transform::new(
            glyph.transform.a,
            glyph.transform.b,
            glyph.transform.c,
            glyph.transform.d,
            0.0,
            0.0,
        );
        let key = (
            glyph.outline_key,
            [linear.a, linear.b, linear.c, linear.d].map(f64::to_bits),
        );
        let hull = self.hull(key, outline, linear)?;
        let origin = Point::new(glyph.transform.e, glyph.transform.f);
        let low = Point::new(hull[0].x + origin.x, hull[0].y + origin.y);
        let high = Point::new(hull[1].x + origin.x, hull[1].y + origin.y);
        if !low.is_finite() || !high.is_finite() {
            return Err(Error::syntax("path arithmetic overflow"));
        }
        self.marks.charge(1)?;
        self.bytes.charge_items::<Placement>(1)?;
        let sources = &mut self.glyphs.sources;
        let Some(Node::Glyphs(glyphs)) = self.nodes.get_mut(node.index()) else {
            return Err(Error::invalid_argument("glyphs node expected"));
        };
        let source = if let Some(&source) = sources.get(&key) {
            source
        } else {
            let source = u32::try_from(glyphs.sources.len())
                .map_err(|_| Error::limit("glyph source count exceeded"))?;
            self.bytes.charge_items::<GlyphSource>(1)?;
            self.bytes
                .charge_items::<Segment>(outline.segments().len())?;
            glyphs.sources.push(GlyphSource {
                outline: Arc::clone(outline),
                key: glyph.outline_key,
                linear,
            });
            sources.insert(key, source);
            source
        };
        let index = u32::try_from(glyphs.placements.len())
            .map_err(|_| Error::limit("glyph placement count exceeded"))?;
        if glyphs
            .positions
            .last()
            .is_none_or(|&(_, position)| position != glyph.position)
        {
            glyphs.positions.push((index, glyph.position));
        }
        glyphs.placements.push(Placement { source, origin });
        Ok(())
    }

    /// The paint, mark and glyphs node of the run's next filled glyph: the
    /// run's first interns them, and a full node gives way to a new one.
    fn target(&mut self, run: &GlyphRun<'_>, target: &mut Option<Target>) -> Result<Target> {
        Ok(match *target {
            Some(current) if self.placements(current.node) < MAX_PLACEMENTS => current,
            Some(current) => {
                let node = self.glyphs_node(current.paint, current.mark)?;
                *target.insert(Target { node, ..current })
            }
            None => {
                let state = run
                    .paint
                    .fill_state
                    .as_ref()
                    .ok_or(Error::invalid_argument("filled glyphs without a state"))?;
                let paint = self.paint(run.paint.fill)?;
                let mark = self.mark(state)?;
                let node = self.glyphs_node(paint, mark)?;
                *target.insert(Target { paint, mark, node })
            }
        })
    }

    /// The hull of the source `key`, computed once per recording.
    fn hull(&mut self, key: SourceKey, outline: &Path, linear: Transform) -> Result<[Point; 2]> {
        if let Some(hull) = self.glyphs.hulls.get(&key) {
            return Ok(*hull);
        }
        let hull = hull(outline, linear)?;
        self.bytes.charge_items::<(SourceKey, [Point; 2])>(1)?;
        self.glyphs.hulls.insert(key, hull);
        Ok(hull)
    }

    /// The placements of the glyphs node `node`.
    fn placements(&self, node: NodeId) -> usize {
        match self.nodes.get(node.index()) {
            Some(Node::Glyphs(glyphs)) => glyphs.placements.len(),
            _ => MAX_PLACEMENTS,
        }
    }

    /// The glyphs node a placement with `paint` and `mark` extends: the
    /// open one when it is the current group's last node and matches.
    fn glyphs_node(&mut self, paint: PaintId, mark: MarkInfo) -> Result<NodeId> {
        let group = self.current()?;
        let last = self
            .groups
            .get(group.index())
            .and_then(|g| g.children.last().copied());
        if let Some(node) = &self.glyphs.open
            && last == Some(*node)
            && let Some(Node::Glyphs(glyphs)) = self.nodes.get(node.index())
            && glyphs.paint == paint
            && glyphs.mark.state == mark.state
            && glyphs.mark.clip == mark.clip
            && glyphs.placements.len() < MAX_PLACEMENTS
        {
            return Ok(*node);
        }
        let node = self.push(
            Node::Glyphs(GlyphsNode {
                sources: Vec::new(),
                placements: Vec::new(),
                positions: Vec::new(),
                paint,
                mark,
            }),
            0,
        )?;
        self.glyphs.open = Some(node);
        self.glyphs.sources.clear();
        Ok(node)
    }

    /// A stroked glyph: its outline in default user space under the run's
    /// pen, with `composite` in place of the stroke state's.
    fn stroke_glyph(
        &mut self,
        run: &GlyphRun<'_>,
        glyph: &Glyph,
        outline: &Path,
        composite: Option<&Arc<Composite>>,
    ) -> Result<()> {
        let (paint, pen) = run
            .paint
            .stroke
            .ok_or(Error::invalid_argument("stroked glyphs without a pen"))?;
        let mut state = run
            .paint
            .stroke_state
            .ok_or(Error::invalid_argument("stroked glyphs without a state"))?;
        if let Some(composite) = composite {
            state.composite = composite;
        }
        let paint = self.paint(Some(paint))?;
        let mark = self.mark(&state)?;
        let path = Arc::new(user_path(outline, glyph.transform)?);
        self.stroke_node(&path, pen, paint, mark)
    }

    /// A glyph filled and stroked: a group of a fill and a stroke of its
    /// outline, as `B` paints a path (11.7.4.4).
    fn fill_stroke_glyph(
        &mut self,
        run: &GlyphRun<'_>,
        glyph: &Glyph,
        outline: &Path,
    ) -> Result<()> {
        let (Some(fill_state), Some(stroke_state)) = (run.paint.fill_state, run.paint.stroke_state)
        else {
            return Err(Error::invalid_argument(
                "filled and stroked glyphs without states",
            ));
        };
        let key = (
            self.interner.state(fill_state.composite, &mut self.bytes)?,
            self.interner
                .state(stroke_state.composite, &mut self.bytes)?,
        );
        let states = if let Some(states) = self.glyphs.derived.get(&key) {
            states.clone()
        } else {
            let opaque = Arc::new(Composite::default());
            let states = FillStroke::new(fill_state.composite, stroke_state.composite, &opaque);
            self.glyphs.derived.insert(key, states.clone());
            states
        };
        let path = Arc::new(user_path(outline, glyph.transform)?);
        let mut group_state = fill_state;
        group_state.composite = &states.group;
        let mark = self.mark(&group_state)?;
        self.open(
            Group {
                role: GroupRole::Transparency,
                cause: GroupCause::FillStroke,
                attributes: GroupAttributes {
                    knockout: states.knockout,
                    ..GroupAttributes::default()
                },
                bbox: None,
                view: None,
                children: Vec::new(),
                mark,
            },
            super::Open::Group,
        )?;
        let mut member = fill_state;
        member.composite = &states.fill;
        let paint = self.paint(run.paint.fill)?;
        let mark = self.mark(&member)?;
        self.fill_node(&path, FillRule::NonZero, paint, mark)?;
        self.stroke_glyph(run, glyph, outline, Some(&states.stroke))?;
        self.close(super::Open::Group)
    }
}

/// A glyph outline in default user space, every point finite.
fn user_path(outline: &Path, transform: Transform) -> Result<Path> {
    let path = outline.transformed(transform);
    for segment in path.segments() {
        let points: &[Point] = match segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => std::slice::from_ref(p),
            Segment::CubicTo { c1, c2, end } => &[*c1, *c2, *end],
            Segment::Close => &[],
        };
        if points.iter().any(|p| !p.is_finite()) {
            return Err(Error::syntax("path arithmetic overflow"));
        }
    }
    Ok(path)
}

/// The box of every on- and off-curve point of `outline` under `linear`.
fn hull(outline: &Path, linear: Transform) -> Result<[Point; 2]> {
    let mut low = Point::new(f64::INFINITY, f64::INFINITY);
    let mut high = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut record = |p: Point| -> Result<()> {
        let p = p * linear;
        if !p.is_finite() {
            return Err(Error::syntax("text matrix arithmetic overflow"));
        }
        low = Point::new(low.x.min(p.x), low.y.min(p.y));
        high = Point::new(high.x.max(p.x), high.y.max(p.y));
        Ok(())
    };
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => record(p)?,
            Segment::CubicTo { c1, c2, end } => {
                record(c1)?;
                record(c2)?;
                record(end)?;
            }
            Segment::Close => {}
        }
    }
    Ok([low, high])
}
