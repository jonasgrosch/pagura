//! The scene recorder: a device that records a [`Scene`].

mod glyphs;
mod intern;

use crate::device::{
    Device, FillMark, GlyphRun, GroupInfo, ImageMark, MarkState, Needs, PageInfo, ShadingMark,
    SoftMaskInfo, StrokeMark, TilingInfo,
};
use crate::model::{GroupAttributes, GroupCause, MaskId, Paint, PatternId};
use crate::scene::{
    FillNode, Group, GroupId, GroupRole, ImageNode, MarkInfo, Node, NodeId, PageSpace, PaintId,
    Scene, ShadingNode, SoftMask, StrokeNode, Tiling,
};
use intern::Interner;
use pagura_core::{Error, Limits, Meter, Result};
use pagura_raster::{FillRule, Path};
use std::sync::Arc;

/// Records the marks a page's interpretation emits into a [`Scene`],
/// interning paints, states, pens and clips, and charging `display_commands`
/// for every mark (each glyph placement included) and `render_bytes` for
/// the scene's storage.
///
/// Consecutive glyph runs with the same paint, state and clip share one
/// glyphs node; each placement remains a separate mark (9.3.8). Stroked
/// glyphs become stroke nodes, and glyphs filled and stroked become a
/// group of a fill and a stroke, as `B` paints a path.
#[derive(Debug)]
pub struct SceneRecorder {
    limits: Limits,
    marks: Meter,
    bytes: Meter,
    page: Option<(PageSpace, bool)>,
    root: GroupId,
    nodes: Vec<Node>,
    groups: Vec<Group>,
    masks: Vec<SoftMask>,
    tilings: Vec<Tiling>,
    interner: Interner,
    /// Open groups, innermost last.
    stack: Vec<(Open, GroupId)>,
    glyphs: glyphs::Glyphs,
    finished: bool,
}

/// What an open group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Page,
    Group,
    Mask,
    Tiling,
}

impl SceneRecorder {
    /// An empty recorder under `limits`.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            marks: Meter::new(limits.display_commands, "display list budget exceeded"),
            bytes: Meter::new(limits.render_bytes, "scene byte budget exceeded"),
            page: None,
            root: GroupId(0),
            nodes: Vec::new(),
            groups: Vec::new(),
            masks: Vec::new(),
            tilings: Vec::new(),
            interner: Interner::default(),
            stack: Vec::new(),
            glyphs: glyphs::Glyphs::default(),
            finished: false,
        }
    }

    /// The recorded scene.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when no complete page was recorded.
    pub fn finish(self) -> Result<Scene> {
        let Some((page, print_intent)) = self.page.filter(|_| self.finished) else {
            return Err(Error::invalid_argument("no complete page was recorded"));
        };
        let (clips, states, paints, pens) = self.interner.into_tables();
        Ok(Scene {
            page,
            print_intent,
            root: self.root,
            nodes: self.nodes,
            groups: self.groups,
            masks: self.masks,
            tilings: self.tilings,
            clips,
            states,
            paints,
            pens,
            bytes: self.bytes.used(),
        })
    }

    /// The group content goes into.
    fn current(&self) -> Result<GroupId> {
        match self.stack.last() {
            Some(&(_, id)) if !self.finished => Ok(id),
            _ => Err(Error::invalid_argument("mark outside a recorded page")),
        }
    }

    /// The state, clip and position of a mark, interned.
    fn mark(&mut self, state: &MarkState<'_>) -> Result<MarkInfo> {
        let state_id = self.interner.state(state.composite, &mut self.bytes)?;
        let clip = state
            .clip
            .map(|clip| self.interner.clip(clip, &mut self.bytes))
            .transpose()?;
        Ok(MarkInfo {
            state: state_id,
            clip,
            position: state.position,
        })
    }

    fn paint(&mut self, paint: Option<&Arc<Paint>>) -> Result<PaintId> {
        let paint = paint.ok_or(Error::invalid_argument("mark without paint"))?;
        self.interner.paint(paint, &mut self.bytes)
    }

    /// Appends `node` to the current group, charging `marks` marks.
    fn push(&mut self, node: Node, marks: usize) -> Result<NodeId> {
        let group = self.current()?;
        self.marks.charge(marks)?;
        self.bytes.charge_items::<Node>(1)?;
        self.bytes.charge_items::<NodeId>(1)?;
        let id = NodeId(
            u32::try_from(self.nodes.len())
                .map_err(|_| Error::limit("scene node count exceeded"))?,
        );
        self.nodes
            .try_reserve(1)
            .map_err(|_| Error::limit("scene allocation failed"))?;
        self.nodes.push(node);
        let children = &mut self
            .groups
            .get_mut(group.index())
            .ok_or(Error::invalid_argument("unknown scene group"))?
            .children;
        children
            .try_reserve(1)
            .map_err(|_| Error::limit("scene allocation failed"))?;
        children.push(id);
        Ok(id)
    }

    fn charge_path(&mut self, path: &Path) -> Result<()> {
        self.bytes
            .charge_items::<pagura_raster::Segment>(path.segments().len())
    }

    /// Opens a group of `role`, as a node of the current group for a
    /// transparency group.
    fn open(&mut self, group: Group, open: Open) -> Result<GroupId> {
        let id = GroupId(
            u32::try_from(self.groups.len())
                .map_err(|_| Error::limit("scene group count exceeded"))?,
        );
        if open == Open::Group {
            self.push(Node::Group(id), 0)?;
        }
        if let Some(bbox) = &group.bbox {
            self.charge_path(bbox)?;
        }
        self.bytes.charge_items::<Group>(1)?;
        self.groups
            .try_reserve(1)
            .map_err(|_| Error::limit("scene allocation failed"))?;
        self.groups.push(group);
        self.stack.push((open, id));
        Ok(id)
    }

    fn close(&mut self, open: Open) -> Result<()> {
        if self.stack.last().map(|&(kind, _)| kind) != Some(open) {
            return Err(Error::invalid_argument("unbalanced scene group"));
        }
        self.stack.pop();
        Ok(())
    }

    fn fill_node(
        &mut self,
        path: &Arc<Path>,
        rule: FillRule,
        paint: PaintId,
        mark: MarkInfo,
    ) -> Result<()> {
        self.charge_path(path)?;
        self.push(
            Node::Fill(FillNode {
                path: Arc::clone(path),
                rule,
                paint,
                mark,
            }),
            1,
        )?;
        Ok(())
    }

    fn stroke_node(
        &mut self,
        path: &Arc<Path>,
        pen: &Arc<crate::model::Pen>,
        paint: PaintId,
        mark: MarkInfo,
    ) -> Result<()> {
        self.charge_path(path)?;
        let pen = self.interner.pen(pen, &mut self.bytes)?;
        self.push(
            Node::Stroke(StrokeNode {
                path: Arc::clone(path),
                pen,
                paint,
                mark,
            }),
            1,
        )?;
        Ok(())
    }
}

impl Device for SceneRecorder {
    fn needs(&self) -> Needs {
        Needs::SCENE
    }

    fn begin_page(&mut self, page: &PageInfo) -> Result<()> {
        if self.page.is_some() {
            return Err(Error::invalid_argument("the recorder holds a page"));
        }
        self.page = Some((PageSpace::of(page), page.print_intent));
        let state = self.interner.state(
            &Arc::new(crate::model::Composite::default()),
            &mut self.bytes,
        )?;
        self.root = self.open(
            Group {
                role: GroupRole::Page,
                cause: GroupCause::Document,
                attributes: page.group.unwrap_or_default(),
                bbox: None,
                view: None,
                children: Vec::new(),
                mark: MarkInfo {
                    state,
                    clip: None,
                    position: pagura_core::Position::Unknown,
                },
            },
            Open::Page,
        )?;
        Ok(())
    }

    fn end_page(&mut self) -> Result<()> {
        self.close(Open::Page)?;
        if !self.stack.is_empty() {
            return Err(Error::invalid_argument("unbalanced scene group"));
        }
        self.finished = true;
        Ok(())
    }

    fn fill(&mut self, mark: &FillMark<'_>) -> Result<()> {
        if mark.state.hidden {
            return Ok(());
        }
        let paint = self.paint(mark.paint)?;
        let info = self.mark(&mark.state)?;
        self.fill_node(mark.path, mark.rule, paint, info)
    }

    fn stroke(&mut self, mark: &StrokeMark<'_>) -> Result<()> {
        if mark.state.hidden {
            return Ok(());
        }
        let paint = self.paint(mark.paint)?;
        let info = self.mark(&mark.state)?;
        self.stroke_node(mark.path, mark.pen, paint, info)
    }

    fn glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        if run.hidden {
            return Ok(());
        }
        self.record_glyphs(run)
    }

    fn image(&mut self, mark: &ImageMark<'_>) -> Result<()> {
        if mark.state.hidden {
            return Ok(());
        }
        let stencil = mark
            .stencil
            .map(|paint| self.paint(Some(paint)))
            .transpose()?;
        let info = self.mark(&mark.state)?;
        self.bytes.charge(mark.image.source.retained_bytes())?;
        self.push(
            Node::Image(ImageNode {
                image: Arc::clone(mark.image),
                transform: mark.transform,
                stencil,
                mark: info,
            }),
            1,
        )?;
        Ok(())
    }

    fn shading(&mut self, mark: &ShadingMark<'_>) -> Result<()> {
        if mark.state.hidden {
            return Ok(());
        }
        let info = self.mark(&mark.state)?;
        self.bytes
            .charge(crate::resources::shading::retained_bytes(
                &mark.shading.kind,
            ))?;
        self.push(
            Node::Shading(ShadingNode {
                shading: Arc::clone(mark.shading),
                transform: mark.transform,
                mark: info,
            }),
            1,
        )?;
        Ok(())
    }

    fn begin_group(&mut self, group: &GroupInfo<'_>) -> Result<()> {
        let mark = self.mark(&group.state)?;
        self.open(
            Group {
                role: GroupRole::Transparency,
                cause: group.cause,
                attributes: group.attributes,
                bbox: group.bbox.cloned(),
                view: group.view,
                children: Vec::new(),
                mark,
            },
            Open::Group,
        )?;
        Ok(())
    }

    fn end_group(&mut self) -> Result<()> {
        self.close(Open::Group)
    }

    fn begin_soft_mask(&mut self, mask: &SoftMaskInfo<'_>) -> Result<()> {
        if mask.id != MaskId(u32::try_from(self.masks.len()).unwrap_or(u32::MAX)) {
            return Err(Error::invalid_argument(
                "soft masks out of definition order",
            ));
        }
        let mark = self.page_mark()?;
        let group = self.open(
            Group {
                role: GroupRole::SoftMask { kind: mask.kind },
                cause: GroupCause::Document,
                attributes: mask.group,
                bbox: mask.bbox.cloned(),
                view: None,
                children: Vec::new(),
                mark,
            },
            Open::Mask,
        )?;
        self.bytes.charge_items::<SoftMask>(1)?;
        self.masks.push(SoftMask {
            group,
            kind: mask.kind,
            backdrop: mask.backdrop.cloned(),
            transfer: mask.transfer.cloned(),
            position: mask.position,
        });
        Ok(())
    }

    fn end_soft_mask(&mut self) -> Result<()> {
        self.close(Open::Mask)
    }

    fn begin_tiling(&mut self, tiling: &TilingInfo) -> Result<()> {
        if tiling.id != PatternId(u32::try_from(self.tilings.len()).unwrap_or(u32::MAX)) {
            return Err(Error::invalid_argument(
                "tiling patterns out of definition order",
            ));
        }
        let mark = self.page_mark()?;
        let cell = self.open(
            Group {
                role: GroupRole::Tiling,
                cause: GroupCause::Document,
                attributes: GroupAttributes::default(),
                bbox: Some(Arc::new(Path::rect(tiling.bbox))),
                view: None,
                children: Vec::new(),
                mark,
            },
            Open::Tiling,
        )?;
        self.bytes.charge_items::<Tiling>(1)?;
        self.tilings.push(Tiling {
            cell,
            bbox: tiling.bbox,
            step: tiling.step,
            matrix: tiling.matrix,
            colors: tiling.colors,
        });
        Ok(())
    }

    fn end_tiling(&mut self) -> Result<()> {
        self.close(Open::Tiling)
    }
}

impl SceneRecorder {
    /// The mark information of groups that are not composited where they
    /// occur: the page's.
    fn page_mark(&self) -> Result<MarkInfo> {
        self.groups
            .get(self.root.index())
            .map(|group| group.mark)
            .ok_or(Error::invalid_argument("mark outside a recorded page"))
    }

    /// The limits the recorder charges.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
}

#[cfg(test)]
mod tests;
