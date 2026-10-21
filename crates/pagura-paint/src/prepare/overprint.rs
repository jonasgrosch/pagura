//! Overprint simulation (8.6.7, 11.7.4, ADR 0004 §7), decided from the
//! page's whole content: whether it is needed, and the spot colorants that
//! become channels of every buffer, each with its first painting
//! definition for the preview at output.

use crate::request::OverprintMode;
use pagura_color::{ColorSpace, Colorant};
use pagura_core::{Error, Name, Result};
use pagura_interpret::{Composite, GroupId, GroupRole, Node, Paint, PatternId, Scene};
use std::collections::HashSet;
use std::sync::Arc;

/// The separations of a page.
#[derive(Debug, Default)]
pub(crate) struct Separations {
    /// Overprinting is simulated.
    pub(crate) simulate: bool,
    /// Spot colorants in discovery order, with their first definition.
    pub(crate) spots: Vec<(Name, Arc<ColorSpace>)>,
}

/// Discovers the separations of `scene` under `mode`; content nested deeper
/// than `depth` is a `Limit` error.
///
/// # Errors
///
/// A `Limit` error beyond 32 spot colorants or `depth`.
pub(crate) fn discover(scene: &Scene, mode: OverprintMode, depth: usize) -> Result<Separations> {
    if mode == OverprintMode::Ignore {
        return Ok(Separations::default());
    }
    let mut walk = Walk {
        scene,
        depth,
        cells: HashSet::new(),
        overprint: false,
        tint: false,
        nonzero: false,
        spots: Vec::new(),
    };
    walk.group(scene.root, 0)?;
    if !(walk.nonzero || walk.overprint && walk.tint) {
        return Ok(Separations::default());
    }
    Ok(Separations {
        simulate: true,
        spots: walk.spots,
    })
}

struct Walk<'s> {
    scene: &'s Scene,
    depth: usize,
    cells: HashSet<PatternId>,
    overprint: bool,
    tint: bool,
    nonzero: bool,
    spots: Vec<(Name, Arc<ColorSpace>)>,
}

impl Walk<'_> {
    fn group(&mut self, id: GroupId, depth: usize) -> Result<()> {
        if depth >= self.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        let scene = self.scene;
        let group = scene
            .group(id)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        for &child in &group.children {
            let node = scene
                .node(child)
                .ok_or(Error::invalid_argument("scene node out of range"))?;
            let state = |mark: &pagura_interpret::MarkInfo| {
                scene
                    .states
                    .get(mark.state.index())
                    .ok_or(Error::invalid_argument("scene state out of range"))
            };
            let paint = |id: pagura_interpret::PaintId| {
                scene
                    .paints
                    .get(id.index())
                    .ok_or(Error::invalid_argument("scene paint out of range"))
            };
            match node {
                Node::Fill(fill) => self.paint(paint(fill.paint)?, state(&fill.mark)?, depth)?,
                Node::Stroke(stroke) => {
                    self.paint(paint(stroke.paint)?, state(&stroke.mark)?, depth)?;
                }
                Node::Glyphs(glyphs) => {
                    self.paint(paint(glyphs.paint)?, state(&glyphs.mark)?, depth)?;
                }
                Node::Image(image) => match (&image.image.space, image.stencil) {
                    (Some(space), _) => self.space(space, state(&image.mark)?, false)?,
                    (None, Some(stencil)) => {
                        self.paint(paint(stencil)?, state(&image.mark)?, depth)?;
                    }
                    (None, None) => {}
                },
                Node::Shading(shading) => {
                    self.space(&shading.shading.space, state(&shading.mark)?, false)?;
                }
                Node::Group(id) => {
                    let role = scene.group(*id).map(|g| g.role);
                    if role == Some(GroupRole::Transparency) {
                        self.group(*id, depth + 1)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn paint(&mut self, paint: &Paint, state: &Composite, depth: usize) -> Result<()> {
        match paint {
            Paint::Solid(color) => self.space(&color.space, state, true),
            Paint::Shading(shading) => self.space(&shading.shading.space, state, false),
            Paint::Tiling(tiling) => {
                self.overprint |= state.overprint.enabled;
                if self.cells.insert(tiling.pattern) {
                    let cell = self
                        .scene
                        .tilings
                        .get(crate::prepare::index(tiling.pattern.0))
                        .ok_or(Error::invalid_argument("scene pattern out of range"))?
                        .cell;
                    self.group(cell, depth + 1)?;
                }
                Ok(())
            }
        }
    }

    fn space(&mut self, space: &ColorSpace, state: &Composite, direct: bool) -> Result<()> {
        if !space.paints() {
            return Ok(());
        }
        let mut space = space;
        while let ColorSpace::Indexed(indexed) = space {
            space = &indexed.base;
        }
        let enabled = state.overprint.enabled;
        self.overprint |= enabled;
        self.nonzero |= matches!(space, ColorSpace::DeviceCmyk)
            && enabled
            && state.overprint.nonzero
            && direct;
        let colorants: &[Colorant] = match space {
            ColorSpace::Separation(separation) => std::slice::from_ref(&separation.colorant),
            ColorSpace::DeviceN(device) => &device.colorants,
            _ => return Ok(()),
        };
        self.tint = true;
        for colorant in colorants {
            let Colorant::Named(name) = colorant else {
                continue;
            };
            if process(name) || self.spots.iter().any(|(spot, _)| spot == name) {
                continue;
            }
            if self.spots.len() >= crate::plan::MAX_SPOTS {
                return Err(Error::limit("page spot colourant budget exceeded"));
            }
            self.spots.push((name.clone(), Arc::new(space.clone())));
        }
        Ok(())
    }
}

/// Whether `name` is a process colorant.
pub(crate) fn process(name: &Name) -> bool {
    [b"Cyan".as_slice(), b"Magenta", b"Yellow", b"Black"]
        .iter()
        .any(|&n| name == n)
}
