//! Display separation policy and compatible overprinting (8.6.7; 11.7.3–11.7.4).
//!
//! The simulated device has CMYK process inks and up to 32 named spot inks.
//! Spot planes use four-channel batches through the same f64 group compositor;
//! process conversions never touch them. At output, alternate tint transforms
//! supply CMYK ink equivalents, added with saturation to simulate ink on paper.
//! The first painting definition of a name supplies its display tint transform;
//! channels from the same DeviceN definition are evaluated together. This final
//! approximation is device policy, not a physical ink prediction.

use super::{Context, Source};
use crate::color::{Color, Space};
use crate::display::{Command, PathCommand};
use crate::transparency::{Composite, Conversion, Model};
use pagura_core::{Error, Limits, Name, Result};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Immutable page requirements, shared by every tile of a display list.
#[derive(Debug, Default)]
pub(crate) struct Separations {
    pub(crate) enabled: bool,
    spots: Vec<Name>,
    transforms: Vec<TintTransform>,
}
#[derive(Debug)]
struct TintTransform {
    space: Space,
    inputs: Vec<(usize, usize)>,
}
struct Spot {
    name: Name,
    space: Space,
}
#[derive(Default)]
struct Discovery {
    overprint: bool,
    tint: bool,
    nonzero: bool,
    spots: Vec<Spot>,
    patterns: BTreeSet<usize>,
}
#[derive(Default)]
pub(super) struct Preview {
    last: Option<([f64; 32], [f64; 4])>,
}

impl Separations {
    pub(crate) fn new(commands: &[Command], limits: Limits) -> Result<Self> {
        let mut found = Discovery::default();
        let mut work = limits.display_commands;
        found.visit(commands, limits.depth, &mut work)?;
        if !(found.nonzero || found.overprint && found.tint) {
            return Ok(Self::default());
        }
        if found.spots.len() > 32 {
            return Err(Error::limit("page spot colourant budget exceeded"));
        }
        let transforms = transforms(&found.spots);
        Ok(Self {
            enabled: true,
            spots: found.spots.into_iter().map(|s| s.name).collect(),
            transforms,
        })
    }
    pub(super) fn batches(&self) -> usize {
        self.spots.len().div_ceil(4)
    }
    pub(super) fn len(&self) -> usize {
        self.spots.len()
    }
    fn equivalent(&self, tints: &[f64; 32], work: &mut usize) -> Result<[f64; 4]> {
        let mut output = [1.0; 4];
        for transform in &self.transforms {
            let mut input = [0.0; 32];
            for &(i, spot) in &transform.inputs {
                input[i] = tints[spot];
            }
            if input.iter().all(|&v| v == 0.0) {
                continue;
            }
            let equivalent = Model::Cmyk.source(
                &transform.space,
                &input[..transform.space.components()],
                &Conversion::default(),
                work,
            )?;
            for (out, value) in output.iter_mut().zip(equivalent) {
                *out = (*out + value - 1.0).max(0.0);
            }
        }
        Ok(output)
    }
}

// Resolve name and definition identity once; varying pixel tints use only indices.
fn transforms(spots: &[Spot]) -> Vec<TintTransform> {
    let mut transforms = Vec::new();
    let mut used = [false; 32];
    for (index, spot) in spots.iter().enumerate() {
        if used[index] {
            continue;
        }
        let Space::Tint {
            names, function, ..
        } = &spot.space
        else {
            continue;
        };
        let inputs = names.iter().enumerate().filter_map(|(i, name)| {
            let j = spots.iter().position(|s| s.name == *name
                && matches!(&s.space, Space::Tint { function: f, .. } if Arc::ptr_eq(f, function)))?;
            used[j] = true;
            Some((i, j))
        }).collect();
        transforms.push(TintTransform {
            space: spot.space.clone(),
            inputs,
        });
    }
    transforms
}

impl Discovery {
    fn visit(&mut self, commands: &[Command], depth: usize, work: &mut usize) -> Result<()> {
        *work = work
            .checked_sub(commands.len())
            .ok_or(Error::limit("colourant traversal budget exceeded"))?;
        let remaining = depth
            .checked_sub(1)
            .ok_or(Error::limit("colourant nesting exceeded"))?;
        for command in commands {
            match command {
                Command::Path(p) => self.path(p, remaining, work)?,
                Command::Glyphs(run) => self.path(&run.paint, remaining, work)?,
                Command::Image(i) => {
                    if let Some(space) = &i.image.space {
                        self.paint(space, &i.composite, false);
                    } else if let Color::Solid { space, .. } = &i.stencil {
                        self.paint(space, &i.composite, true);
                    }
                }
                Command::Shading(s) => self.paint(&s.shading.space, &s.composite, false),
                Command::Group(g) => self.visit(&g.commands, remaining, work)?,
            }
        }
        Ok(())
    }
    /// A path or glyph-run paint, including the content of its pattern.
    fn path(&mut self, p: &PathCommand, remaining: usize, work: &mut usize) -> Result<()> {
        match &p.color {
            Color::Solid { space, .. } => self.paint(space, &p.composite, true),
            Color::Pattern {
                selected: Some(pattern),
                ..
            } => {
                self.overprint |= p.composite.overprint;
                if self.patterns.insert(Arc::as_ptr(pattern) as usize) {
                    match &pattern.kind {
                        crate::pattern::Kind::Tiling(t) => {
                            self.visit(&t.commands, remaining, work)?;
                        }
                        crate::pattern::Kind::Shading(s) => {
                            self.paint(&s.space, &p.composite, false);
                        }
                    }
                }
            }
            Color::Pattern { .. } => {}
        }
        Ok(())
    }
    fn paint(&mut self, mut space: &Space, state: &Composite, direct: bool) {
        if !space.paints() {
            return;
        }
        while let Space::Indexed { base, .. } = space {
            space = base;
        }
        self.overprint |= state.overprint;
        self.nonzero |=
            matches!(space, Space::Cmyk) && state.overprint && state.nonzero_overprint && direct;
        self.tint |= matches!(space, Space::Tint { .. } | Space::Special { all: true, .. });
        if let Space::Tint { names, .. } = space {
            for name in names.iter() {
                if process(name).is_some()
                    || name == b"None"
                    || self.spots.iter().any(|s| s.name == *name)
                {
                    continue;
                }
                if self.spots.len() > 32 {
                    continue;
                }
                self.spots.push(Spot {
                    name: name.clone(),
                    space: space.clone(),
                });
            }
        }
    }
}

impl Preview {
    pub(super) fn rgb(
        &mut self,
        plan: &Separations,
        model: Model,
        color: [f64; 4],
        tints: &[f64; 32],
        work: &mut usize,
    ) -> Result<[f64; 3]> {
        let process = if model == Model::Cmyk {
            color
        } else {
            Model::Cmyk.encode_rgb(model.rgb(color), &Conversion::default(), work)?
        };
        // The cache compares exact inputs; it never quantizes a tint function.
        let equivalent = if let Some((previous, equivalent)) = self.last
            && previous == *tints
        {
            equivalent
        } else {
            let equivalent = plan.equivalent(tints, work)?;
            self.last = Some((*tints, equivalent));
            equivalent
        };
        Ok(Model::Cmyk.rgb([0, 1, 2, 3].map(|i| (process[i] + equivalent[i] - 1.0).max(0.0))))
    }
}

fn process(name: &Name) -> Option<usize> {
    [b"Cyan".as_slice(), b"Magenta", b"Yellow", b"Black"]
        .iter()
        .position(|&n| name == n)
}

impl Context {
    pub(super) fn color(
        &mut self,
        color: &Color,
        model: Model,
        state: &Composite,
        direct: bool,
    ) -> Result<Source> {
        let Color::Solid { space, components } = color else {
            return Err(Error::invalid_argument("pattern is not a solid colour"));
        };
        self.source(space, components, model, state, direct)
    }
    pub(super) fn source(
        &mut self,
        space: &Space,
        components: &[f64],
        model: Model,
        state: &Composite,
        direct: bool,
    ) -> Result<Source> {
        if let Some((base, decoded)) = crate::transparency::indexed(space, components)? {
            return self.source(base, &decoded, model, state, direct);
        }
        let mut result = self.sampled_source(space, model, state);
        let native = self.native_tint(space, model);
        result.color = if result.spot || native.is_some() {
            [1.0; 4]
        } else {
            model.source(
                space,
                components,
                &state.conversion,
                &mut self.function_work,
            )?
        };
        if let Space::Special { all: true, .. } = space
            && result.spot
        {
            result.color = [1.0 - components.first().copied().unwrap_or(1.0).clamp(0.0, 1.0); 4];
        }
        if let Some(names) = native {
            for (name, &tint) in names.iter().zip(components) {
                if let Some(i) = self.tint_index(name) {
                    result.color[i] = 1.0 - tint.clamp(0.0, 1.0);
                }
            }
        }
        if self.overprint == crate::overprint::Policy::Simulate
            && model == Model::Cmyk
            && !result.spot
            && direct
            && state.overprint
            && state.nonzero_overprint
            && let Some(native) = model.native(space, components)
        {
            result.preserve = native.zero;
        }
        Ok(result)
    }
    /// Sampled images and shadings have value-independent component selection.
    /// In particular this never probes a tint function at an unused input.
    pub(super) fn sampled_source(
        &self,
        mut space: &Space,
        model: Model,
        state: &Composite,
    ) -> Source {
        while let Space::Indexed { base, .. } = space {
            space = base;
        }
        let mut result = Source {
            spot: self.spot_batch.is_some(),
            ..Source::new([0.0; 4], 1.0, 1.0)
        };
        if !state.overprint {
            return result;
        }
        let mut specified = [!result.spot; 4];
        if let Some(names) = self.native_tint(space, model) {
            specified = [false; 4];
            for name in names {
                if let Some(i) = self.tint_index(name) {
                    specified[i] = true;
                }
            }
        } else if matches!(space, Space::Special { all: true, .. }) {
            specified = [true; 4];
        }
        result.preserve = specified.map(|v| !v);
        result
    }
    fn native_tint<'a>(&self, space: &'a Space, model: Model) -> Option<&'a [Name]> {
        if self.separations.enabled
            && !self.mask_colors
            && let Space::Tint { names, .. } = space
            && (model == Model::Cmyk || names.iter().all(|n| process(n).is_none()))
        {
            Some(names)
        } else {
            None
        }
    }
    fn tint_index(&self, name: &Name) -> Option<usize> {
        if let Some(batch) = self.spot_batch {
            self.separations
                .spots
                .iter()
                .position(|s| *s == *name)
                .and_then(|i| i.checked_sub(batch * 4))
                .filter(|&i| i < 4)
        } else {
            process(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_patterns_are_visited_once_within_the_work_budget() {
        use std::sync::Arc;
        let mut path = pagura_raster::PathBuilder::new();
        path.rect(0.0, 0.0, 1.0, 1.0);
        let path = Arc::new(path.finish());
        let command = |color| {
            Command::Path(PathCommand {
                path: Arc::clone(&path),
                mark: crate::display::Mark::Fill(pagura_raster::FillRule::NonZero),
                clip: None,
                color,
                position: pagura_core::Position::Unknown,
                transfer: None,
                composite: Composite {
                    overprint: true,
                    ..Composite::default()
                },
            })
        };
        let mut commands = vec![command(Color::default())];
        // Only two commands are stored at each level, but walking every
        // shared child would visit more than sixteen million paint instances.
        for _ in 0..24 {
            let pattern = Arc::new(crate::pattern::Pattern {
                transform: pagura_raster::Transform::IDENTITY,
                kind: crate::pattern::Kind::Tiling(crate::pattern::Tiling {
                    id: pagura_core::ObjectId::new(1, 0),
                    bbox: pagura_core::Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
                    step: [1.0; 2],
                    uncolored: false,
                    spacing: 2,
                    color: None,
                    commands,
                }),
            });
            commands = (0..2)
                .map(|_| {
                    command(Color::Pattern {
                        base: None,
                        selected: Some(Arc::clone(&pattern)),
                    })
                })
                .collect();
        }
        let limits = Limits {
            display_commands: 128,
            ..Limits::default()
        };
        let mut found = Discovery::default();
        let mut work = limits.display_commands;
        found.visit(&commands, limits.depth, &mut work).unwrap();
        assert_eq!(found.patterns.len(), 24);
        assert!(!Separations::new(&commands, limits).unwrap().enabled);
        let limits = Limits {
            display_commands: 1,
            ..limits
        };
        assert!(
            matches!(Separations::new(&commands, limits), Err(e) if e.kind == pagura_core::ErrorKind::Limit)
        );
    }

    #[test]
    fn repeated_display_tints_do_not_spend_calculator_work() {
        let document = pagura_document::Document::open(
            include_bytes!("../../../../corpus/generated/overprint/devicen-on.pdf").to_vec(),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
        let separations = Separations::new(&list.commands, list.limits).unwrap();
        let mut preview = Preview::default();
        let mut tints = [0.0; 32];
        tints[..2].copy_from_slice(&[0.6, 0.2]);
        let mut work = 0;
        let first = preview
            .rgb(&separations, Model::Cmyk, [1.0; 4], &tints, &mut work)
            .unwrap();
        let spent = work;
        assert!(spent > 0);
        for _ in 0..1000 {
            assert_eq!(
                preview
                    .rgb(&separations, Model::Cmyk, [1.0; 4], &tints, &mut work)
                    .unwrap(),
                first
            );
        }
        assert_eq!(work, spent);
        // Even adjacent f64 tints have separate entries; the cache cannot
        // quantize inputs to a potentially amplifying tint function.
        tints[0] = f64::from_bits(tints[0].to_bits() + 1);
        preview
            .rgb(&separations, Model::Cmyk, [1.0; 4], &tints, &mut work)
            .unwrap();
        assert!(work > spent);
    }
}
