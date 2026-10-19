//! Deferred device transfer selection (11.7.5.2), separate from colour blending.
//!
//! The last elementary object's intrinsic shape selects a transfer; a translucent
//! object or ancestor selects the device default (Identity). Group objects and
//! soft-mask source artwork never select a page transfer. Fractional boundaries
//! involving a nondefault transfer remain unsupported: averaging colours before
//! selecting a nonlinear transfer loses their correlation with spatial regions.

use super::memory::{Held, Memory};
use super::{Context, Frame, Source, Surface};
use crate::Tile;
use crate::display::Command;
use crate::display::{ImageCommand, PathCommand};
use crate::functions::Transfer;
use crate::transparency::Composite;
use crate::transparency::Group;
use pagura_core::{Error, Limits, Meter, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) struct Regions {
    functions: Vec<Transfer>,
    ids: BTreeMap<usize, usize>,
    heads: Vec<usize>,
    tile: Tile,
    pub(super) opaque: bool,
    pub(super) suspended: bool,
    held: Held,
}

const MEMORY: &str = "transfer region memory budget exceeded";

impl Regions {
    pub(super) fn new(
        commands: &[Command],
        limits: Limits,
        memory: &Memory,
    ) -> Result<Option<Self>> {
        let mut walk = Walk {
            functions: BTreeMap::new(),
            cells: BTreeSet::new(),
            work: Meter::new(limits.traversal_work, "transfer traversal budget exceeded"),
            depth: limits.depth,
        };
        walk.collect(commands, 0)?;
        let functions: Vec<_> = walk.functions.into_values().collect();
        if functions.is_empty() {
            return Ok(None);
        }
        let ids = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (Arc::as_ptr(f) as usize, i + 1))
            .collect();
        let mut regions = Self {
            functions,
            ids,
            heads: Vec::new(),
            tile: Tile {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            opaque: true,
            suspended: false,
            held: memory.hold(0, MEMORY)?,
        };
        regions.held.add(regions.bytes(), MEMORY)?;
        Ok(Some(regions))
    }
    pub(super) fn bytes(&self) -> usize {
        self.heads
            .capacity()
            .saturating_mul(size_of::<usize>())
            .saturating_add(
                self.functions
                    .capacity()
                    .saturating_mul(size_of::<Transfer>()),
            )
            .saturating_add(self.ids.len().saturating_mul(64))
    }
    pub(super) fn begin(&mut self, tile: Tile) -> Result<()> {
        self.heads.clear();
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .map_err(|_| Error::limit("transfer region size overflow"))?;
        let added = count
            .saturating_sub(self.heads.capacity())
            .saturating_mul(size_of::<usize>());
        self.held.add(added, MEMORY)?;
        self.heads
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("transfer region allocation failed"))?;
        self.heads.resize(count, 0);
        self.tile = tile;
        Ok(())
    }
    fn record(
        &mut self,
        transfer: Option<&Transfer>,
        opaque: bool,
        point: (u32, u32),
        coverage: f64,
    ) -> Result<()> {
        if self.suspended || coverage <= 0.0 {
            return Ok(());
        }
        let (x, y) = point;
        let x = x.checked_sub(self.tile.x).filter(|&v| v < self.tile.width);
        let y = y.checked_sub(self.tile.y).filter(|&v| v < self.tile.height);
        let index = x.zip(y).and_then(|(x, y)| {
            usize::try_from(u64::from(y) * u64::from(self.tile.width) + u64::from(x)).ok()
        });
        let function = if opaque && self.opaque {
            let id = transfer.map(|f| self.ids.get(&(Arc::as_ptr(f) as usize)).copied());
            debug_assert!(
                id.is_none_or(|id| id.is_some()),
                "the walk finds every transfer a mark can select"
            );
            id.flatten().unwrap_or(0)
        } else {
            0
        };
        let selected = index.and_then(|i| self.heads.get_mut(i));
        debug_assert!(selected.is_some(), "marks lie inside the output region");
        let Some(selected) = selected else {
            return Ok(());
        };
        if coverage < 1.0 && (function != 0 || *selected != 0) {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_TRANSFER,
                "fractional transfer regions require correlated colour coverage",
            ));
        }
        if coverage >= 1.0 {
            *selected = function;
        }
        Ok(())
    }

    pub(super) fn apply(
        &self,
        index: usize,
        color: [f64; 3],
        work: &mut usize,
    ) -> Result<[f64; 3]> {
        let selected = self.heads.get(index).copied();
        debug_assert!(selected.is_some(), "output pixels lie inside the region");
        let Some(functions) = selected
            .unwrap_or(0)
            .checked_sub(1)
            .and_then(|i| self.functions.get(i))
        else {
            return Ok(color);
        };
        let mut mapped = color;
        for (value, function) in mapped.iter_mut().zip(functions.iter()) {
            let mut output = [0.0];
            function.evaluate_into(&[value.clamp(0.0, 1.0)], &mut output, work)?;
            *value = output[0].clamp(0.0, 1.0);
        }
        Ok(mapped)
    }
}

/// One walk over a display list that finds every transfer function. A
/// tiling cell shared by many fills is walked once, and every visited
/// command is charged to `traversal_work`.
///
/// Keys are `Arc` addresses; the display list keeps every keyed `Arc`
/// alive for the whole walk, so no address is reused while it runs.
struct Walk {
    functions: BTreeMap<usize, Transfer>,
    cells: BTreeSet<usize>,
    work: Meter,
    depth: usize,
}

impl Walk {
    fn collect(&mut self, commands: &[Command], depth: usize) -> Result<()> {
        if depth >= self.depth {
            return Err(Error::limit("transfer traversal depth exceeded"));
        }
        self.work.charge(commands.len())?;
        for command in commands {
            let transfer = match command {
                Command::Path(p) => self.paint_transfer(p, depth)?,
                Command::Glyphs(run) => self.paint_transfer(&run.paint, depth)?,
                Command::Image(i) => &i.transfer,
                Command::Shading(s) => &s.transfer,
                Command::Group(g) => {
                    self.collect(&g.commands, depth + 1)?;
                    continue;
                }
            };
            if let Some(transfer) = transfer {
                self.functions
                    .entry(Arc::as_ptr(transfer) as usize)
                    .or_insert_with(|| Arc::clone(transfer));
            }
        }
        Ok(())
    }

    /// A path or glyph-run paint selects its own transfer and any in its
    /// tiling cell.
    fn paint_transfer<'a>(
        &mut self,
        p: &'a PathCommand,
        depth: usize,
    ) -> Result<&'a Option<Transfer>> {
        if let crate::color::Color::Pattern {
            selected: Some(pattern),
            ..
        } = &p.color
            && let crate::pattern::Kind::Tiling(cell) = &pattern.kind
            && self.cells.insert(Arc::as_ptr(pattern) as usize)
        {
            // Cell transfers require pattern-region selection. Discover
            // them so validate rejects the outer pattern explicitly.
            self.collect(&cell.commands, depth + 1)?;
        }
        Ok(&p.transfer)
    }
}

impl Context {
    pub(super) fn begin_transfers(&mut self, tile: Tile) -> Result<()> {
        if let Some(regions) = &mut self.transfers {
            regions.begin(tile)?;
        }
        Ok(())
    }
    pub(super) fn transfer_color(&mut self, index: usize, color: [f64; 3]) -> Result<[f64; 3]> {
        self.transfers.as_ref().map_or(Ok(color), |r| {
            r.apply(index, color, &mut self.function_work)
        })
    }
    pub(super) fn transfer_group(
        &mut self,
        group: &Group,
        surface: &mut Surface,
        members: Frame<'_>,
    ) -> Result<()> {
        let opaque = self.transfers.as_ref().is_none_or(|r| r.opaque);
        if let Some(regions) = &mut self.transfers {
            regions.opaque &= group.composite.simple();
        }
        let result = self.commands(&group.commands, surface, members);
        if let Some(regions) = &mut self.transfers {
            regions.opaque = opaque;
        }
        result
    }
    pub(super) fn image_transfer(
        &mut self,
        command: &ImageCommand,
        point: (u32, u32),
        source: Source,
    ) -> Result<()> {
        self.transfer_mark(
            command.transfer.as_ref(),
            &command.composite,
            point,
            source,
            !matches!(
                command.image.decoded.mask,
                pagura_image::MaskKind::Soft(_)
                    | pagura_image::MaskKind::Embedded(_)
                    | pagura_image::MaskKind::InData
            ),
        )
    }
    pub(super) fn transfer_mark(
        &mut self,
        transfer: Option<&Transfer>,
        state: &Composite,
        point: (u32, u32),
        source: Source,
        eligible: bool,
    ) -> Result<()> {
        let Some(regions) = &mut self.transfers else {
            return Ok(());
        };
        regions.record(transfer, state.simple() && eligible, point, source.shape)?;
        Ok(())
    }

    /// Selects `transfer` for a fully covered run of `count` pixels from `point`.
    pub(super) fn transfer_run(
        &mut self,
        transfer: Option<&Transfer>,
        state: &Composite,
        (x, y): (u32, u32),
        count: usize,
    ) -> Result<()> {
        let Some(regions) = &mut self.transfers else {
            return Ok(());
        };
        let end = u32::try_from(count)
            .ok()
            .and_then(|count| x.checked_add(count))
            .ok_or(Error::limit("mask run exceeds u32"))?;
        for x in x..end {
            regions.record(transfer, state.simple(), (x, y), 1.0)?;
        }
        Ok(())
    }
}

/// Rejects marks whose transfer selection is not implemented: pattern fills,
/// and overprinting marks when overprint is simulated. Under an ignoring
/// policy an overprint flag has no effect, so it cannot matter (8.6.7).
pub(super) fn validate(command: &Command, overprint: crate::overprint::Policy) -> Result<()> {
    let Some(marking) = command.marking() else {
        return Ok(());
    };
    let pattern = matches!(marking.color, Some(crate::color::Color::Pattern { .. }));
    let overprints = marking.composite.overprint && overprint == crate::overprint::Policy::Simulate;
    if overprints || pattern {
        return Err(Error::unsupported(
            crate::features::TRANSPARENCY_TRANSFER,
            "per-colourant and pattern transfer-region selection is not implemented",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;
    use pagura_document::{Document, OpenOptions};
    use pagura_testkit::fixture;

    #[test]
    fn selection_memory_work_and_fractional_boundaries_are_bounded() {
        let document = Document::open(
            fixture("recovery", "transfer-opaque.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
        let regions = |memory: &Memory| {
            Regions::new(&list.commands, Limits::default(), memory)
                .unwrap()
                .unwrap()
        };
        let tile = Tile {
            x: 7,
            y: 9,
            width: 2,
            height: 2,
        };
        let required = regions(&Memory::new(usize::MAX)).bytes() + 4 * size_of::<usize>();
        let short = Memory::new(required - 1);
        assert_eq!(
            regions(&short).begin(tile).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(short.used(), 0);
        let exact = Memory::new(required);
        let mut regions = regions(&exact);
        regions.begin(tile).unwrap();
        assert_eq!(exact.used(), required);
        let function = regions.functions[0].clone();
        let error = regions
            .record(Some(&function), true, (7, 9), 0.5)
            .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(crate::features::TRANSPARENCY_TRANSFER)
        );
        regions.record(Some(&function), true, (7, 9), 1.0).unwrap();
        let mut work = 0;
        assert_eq!(regions.apply(0, [0.25; 3], &mut work).unwrap(), [0.75; 3]);
        assert_eq!(
            regions
                .apply(0, [0.25; 3], &mut Limits::default().function_work)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!(
            regions.record(None, false, (7, 9), 0.5).unwrap_err().kind,
            ErrorKind::Unsupported(crate::features::TRANSPARENCY_TRANSFER)
        );
        regions.suspended = true;
        regions.record(None, false, (7, 9), 1.0).unwrap();
        assert_eq!(regions.apply(0, [0.25; 3], &mut 0).unwrap(), [0.75; 3]);
        regions.suspended = false;
        regions.opaque = false;
        regions.record(Some(&function), true, (7, 9), 1.0).unwrap();
        assert_eq!(regions.apply(0, [0.25; 3], &mut 0).unwrap(), [0.25; 3]);
    }

    #[test]
    fn shared_tiling_cells_are_walked_once_within_the_traversal_budget() {
        use crate::color::Color;
        let path = Arc::new(crate::device::unit_square());
        let transfer: Transfer = Arc::new(std::array::from_fn(|_| {
            Arc::new(crate::functions::Function::identity(Limits::default()))
        }));
        let fill = |color, transfer: Option<Transfer>| {
            Command::Path(PathCommand {
                path: Arc::clone(&path),
                mark: crate::display::Mark::Fill(pagura_raster::FillRule::NonZero),
                clip: None,
                color,
                position: pagura_core::Position::Unknown,
                transfer,
                composite: Composite::default(),
            })
        };
        let mut commands = vec![fill(Color::default(), Some(Arc::clone(&transfer)))];
        // Each level fills twice with the cell of the level below: walking
        // every reference would visit 2^40 cells.
        for _ in 0..40 {
            let pattern = Arc::new(crate::pattern::Pattern {
                transform: pagura_raster::Transform::IDENTITY,
                kind: crate::pattern::Kind::Tiling(crate::pattern::Tiling {
                    bbox: pagura_core::Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
                    step: [1.0; 2],
                    spacing: crate::pattern::TilingType::NoDistortion,
                    commands,
                }),
            });
            commands = (0..2)
                .map(|_| {
                    fill(
                        Color::Pattern {
                            base: None,
                            selected: Some(Arc::clone(&pattern)),
                        },
                        None,
                    )
                })
                .collect();
        }
        let limits = Limits {
            depth: 64,
            ..Limits::default()
        };
        let memory = Memory::new(limits.decoded_bytes);
        let regions = Regions::new(&commands, limits, &memory).unwrap().unwrap();
        assert_eq!(regions.functions.len(), 1);
        let small = Limits {
            traversal_work: 10,
            ..limits
        };
        assert_eq!(
            Regions::new(&commands, small, &memory)
                .err()
                .map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
    }

    #[test]
    fn overprint_flags_matter_for_transfers_only_when_simulated() {
        use crate::overprint::Policy;
        let command = Command::Path(PathCommand {
            path: Arc::new(crate::device::unit_square()),
            mark: crate::display::Mark::Fill(pagura_raster::FillRule::NonZero),
            clip: None,
            color: crate::color::Color::default(),
            position: pagura_core::Position::Unknown,
            transfer: None,
            composite: Composite {
                overprint: true,
                ..Composite::default()
            },
        });
        validate(&command, Policy::Ignore).unwrap();
        assert_eq!(
            validate(&command, Policy::Simulate).unwrap_err().kind,
            ErrorKind::Unsupported(crate::features::TRANSPARENCY_TRANSFER)
        );
    }
}
