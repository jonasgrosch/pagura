//! Overprint simulation (8.6.7, 11.7.4; ADR 0004 overprint amendment).
//!
//! Simulation needs a CMYK process device when some mark overprints with
//! zero DeviceCMYK components under `OPM 1`, or when some mark overprints
//! and some mark paints in a Separation or DeviceN space. Named spot
//! colorants then become channels after the process ones, in the order the
//! page first paints them (at most 32). At output, each spot's first
//! painting definition converts its tints to CMYK ink equivalents, which
//! add to the process inks with saturation before the RGB conversion.

use pagura_color::{ColorSpace, Colorant};
use pagura_core::{Error, Name, Result};
use pagura_interpret::{Composite, GroupRole, Node, Paint, PatternId, Scene};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The most spot colorants simulation keeps.
const MAX_SPOTS: usize = 32;

/// What overprint simulation needs for one page.
#[derive(Debug, Clone, Default)]
pub(crate) struct Simulation {
    /// Whether overprinting is simulated.
    pub(crate) enabled: bool,
    /// The spot colorant channels.
    pub(crate) spots: Arc<[Name]>,
    /// The definitions that convert spot tints for display.
    pub(crate) definitions: Vec<Definition>,
}

/// A Separation or DeviceN space that supplies spot tints' display colour.
#[derive(Debug, Clone)]
pub(crate) struct Definition {
    /// The space.
    pub(crate) space: Arc<ColorSpace>,
    /// Pairs of (colorant index in the space, spot channel index).
    pub(crate) inputs: Vec<(usize, usize)>,
}

impl Simulation {
    /// Discovers what simulating `scene` needs; disabled when `simulate`
    /// is false.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error for more than 32 spot colorants or a scene
    /// nested beyond `depth`.
    pub(crate) fn discover(scene: &Scene, simulate: bool, depth: usize) -> Result<Self> {
        if !simulate {
            return Ok(Self::default());
        }
        let mut found = Discovery::default();
        if let Some(root) = scene.group(scene.root) {
            found.visit(scene, &root.children, depth)?;
        }
        if !(found.nonzero || found.overprint && found.tint) {
            return Ok(Self::default());
        }
        if found.spots.len() > MAX_SPOTS {
            return Err(Error::limit("page spot colourant budget exceeded"));
        }
        let definitions = definitions(&found.spots);
        Ok(Self {
            enabled: true,
            spots: found.spots.into_iter().map(|(name, _)| name).collect(),
            definitions,
        })
    }
}

#[derive(Default)]
struct Discovery {
    overprint: bool,
    nonzero: bool,
    tint: bool,
    spots: Vec<(Name, Arc<ColorSpace>)>,
    patterns: BTreeSet<PatternId>,
}

impl Discovery {
    fn visit(
        &mut self,
        scene: &Scene,
        children: &[pagura_interpret::NodeId],
        depth: usize,
    ) -> Result<()> {
        let depth = depth
            .checked_sub(1)
            .ok_or(Error::limit("colourant nesting exceeded"))?;
        for &id in children {
            let Some(node) = scene.node(id) else {
                continue;
            };
            match node {
                Node::Fill(f) => self.paint(scene, f.paint, f.mark.state, depth)?,
                Node::Stroke(s) => self.paint(scene, s.paint, s.mark.state, depth)?,
                Node::Glyphs(g) => self.paint(scene, g.paint, g.mark.state, depth)?,
                Node::Image(i) => {
                    let state = state(scene, i.mark.state);
                    if let Some(space) = &i.image.space {
                        self.space(space, &state, false);
                    } else if let Some(paint) = i.stencil {
                        self.paint(scene, paint, i.mark.state, depth)?;
                    }
                }
                Node::Shading(s) => {
                    self.space(&s.shading.space, &state(scene, s.mark.state), false);
                }
                Node::Group(g) => {
                    if let Some(group) = scene.group(*g)
                        && group.role == GroupRole::Transparency
                    {
                        self.visit(scene, &group.children, depth)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn paint(
        &mut self,
        scene: &Scene,
        paint: pagura_interpret::PaintId,
        state: pagura_interpret::StateId,
        depth: usize,
    ) -> Result<()> {
        let state = self::state(scene, state);
        let Some(paint) = scene.paints.get(paint.index()) else {
            return Ok(());
        };
        match paint.as_ref() {
            Paint::Solid(color) => self.space(&color.space, &state, true),
            Paint::Shading(shading) => {
                self.overprint |= state.overprint.enabled;
                self.space(&shading.shading.space, &state, false);
            }
            Paint::Tiling(tiling) => {
                self.overprint |= state.overprint.enabled;
                if self.patterns.insert(tiling.pattern)
                    && let Some(cell) = scene
                        .tilings
                        .get(usize::try_from(tiling.pattern.0).unwrap_or(usize::MAX))
                    && let Some(group) = scene.group(cell.cell)
                {
                    self.visit(scene, &group.children, depth)?;
                }
            }
        }
        Ok(())
    }

    fn space(&mut self, space: &Arc<ColorSpace>, state: &Composite, direct: bool) {
        if !space.paints() {
            return;
        }
        let mut space = space;
        while let ColorSpace::Indexed(indexed) = space.as_ref() {
            space = &indexed.base;
        }
        self.overprint |= state.overprint.enabled;
        self.nonzero |= matches!(space.as_ref(), ColorSpace::DeviceCmyk)
            && state.overprint.enabled
            && state.overprint.nonzero
            && direct;
        let colorants: &[Colorant] = match space.as_ref() {
            ColorSpace::Separation(separation) => std::slice::from_ref(&separation.colorant),
            ColorSpace::DeviceN(device_n) => &device_n.colorants,
            _ => return,
        };
        self.tint = true;
        for colorant in colorants {
            let Colorant::Named(name) = colorant else {
                continue;
            };
            if process(name).is_some()
                || self.spots.iter().any(|(n, _)| n == name)
                || self.spots.len() > MAX_SPOTS
            {
                continue;
            }
            self.spots.push((name.clone(), Arc::clone(space)));
        }
    }
}

fn state(scene: &Scene, id: pagura_interpret::StateId) -> Composite {
    scene
        .states
        .get(id.index())
        .map_or_else(Composite::default, |state| state.as_ref().clone())
}

/// The process colorant a name denotes.
pub(crate) fn process(name: &Name) -> Option<usize> {
    [b"Cyan".as_slice(), b"Magenta", b"Yellow", b"Black"]
        .iter()
        .position(|&n| name.as_bytes() == n)
}

/// One definition per first painting space, with the spots it supplies.
fn definitions(spots: &[(Name, Arc<ColorSpace>)]) -> Vec<Definition> {
    let mut used = vec![false; spots.len()];
    let mut definitions = Vec::new();
    for (index, (_, space)) in spots.iter().enumerate() {
        if used[index] {
            continue;
        }
        let colorants: Vec<&Colorant> = match space.as_ref() {
            ColorSpace::Separation(separation) => vec![&separation.colorant],
            ColorSpace::DeviceN(device_n) => device_n.colorants.iter().collect(),
            _ => continue,
        };
        let inputs = colorants
            .iter()
            .enumerate()
            .filter_map(|(i, colorant)| {
                let Colorant::Named(name) = colorant else {
                    return None;
                };
                let j = spots
                    .iter()
                    .position(|(n, s)| n == name && Arc::ptr_eq(s, space))?;
                used[j] = true;
                Some((i, j))
            })
            .collect();
        definitions.push(Definition {
            space: Arc::clone(space),
            inputs,
        });
    }
    definitions
}
