//! Deferred device transfer selection (11.7.5.2), separate from colour blending.
//!
//! The last elementary object's intrinsic shape selects a transfer; a translucent
//! object or ancestor selects the device default (Identity). Group objects and
//! soft-mask source artwork never select a page transfer. Fractional boundaries
//! involving a nondefault transfer remain unsupported: averaging colours before
//! selecting a nonlinear transfer loses their correlation with spatial regions.

use super::{Backdrop, Context, Model, Source, Surface};
use crate::Tile;
use crate::display::Command;
use crate::display::{ImageCommand, PathCommand};
use crate::functions::Transfer;
use crate::transparency::Composite;
use crate::transparency::Group;
use pagura_core::{Error, Limits, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) struct Regions {
    functions: Vec<Transfer>,
    ids: BTreeMap<usize, usize>,
    heads: Vec<usize>,
    tile: Tile,
    pub(super) opaque: bool,
    pub(super) suspended: bool,
}

impl Regions {
    pub(super) fn new(commands: &[Command], limits: Limits) -> Result<Option<Self>> {
        let mut registry = BTreeMap::new();
        collect(commands, &mut registry, limits, 0)?;
        let functions: Vec<_> = registry.into_values().collect();
        if functions.is_empty() {
            return Ok(None);
        }
        let ids = functions
            .iter()
            .enumerate()
            .map(|(i, f)| (Arc::as_ptr(f) as usize, i + 1))
            .collect();
        Ok(Some(Self {
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
        }))
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
    pub(super) fn begin(&mut self, tile: Tile, maximum: usize) -> Result<()> {
        self.heads.clear();
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .map_err(|_| Error::limit("transfer region size overflow"))?;
        let added = count
            .saturating_sub(self.heads.capacity())
            .saturating_mul(size_of::<usize>());
        if self.bytes().saturating_add(added) > maximum {
            return Err(Error::limit("transfer region memory budget exceeded"));
        }
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
        let (Some(x), Some(y)) = (x, y) else {
            return Err(Error::invalid_argument(
                "transfer mark outside output region",
            ));
        };
        let index = usize::try_from(u64::from(y) * u64::from(self.tile.width) + u64::from(x))
            .map_err(|_| Error::limit("transfer region index overflow"))?;
        let function = if opaque && self.opaque {
            transfer
                .map(|f| {
                    self.ids.get(&(Arc::as_ptr(f) as usize)).copied().ok_or(
                        Error::invalid_argument("unknown transfer function selection"),
                    )
                })
                .transpose()?
                .unwrap_or(0)
        } else {
            0
        };
        let selected = self
            .heads
            .get_mut(index)
            .ok_or(Error::invalid_argument("invalid transfer pixel"))?;
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
        let selected = *self
            .heads
            .get(index)
            .ok_or(Error::invalid_argument("invalid transfer pixel"))?;
        let Some(functions) = selected.checked_sub(1).and_then(|i| self.functions.get(i)) else {
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

fn collect(
    commands: &[Command],
    functions: &mut BTreeMap<usize, Transfer>,
    limits: Limits,
    depth: usize,
) -> Result<()> {
    if depth >= limits.depth {
        return Err(Error::limit("transfer traversal depth exceeded"));
    }
    for command in commands {
        let transfer = match command {
            Command::Path(p) => paint_transfer(p, functions, limits, depth)?,
            Command::Glyphs(run) => paint_transfer(&run.paint, functions, limits, depth)?,
            Command::Image(i) => &i.transfer,
            Command::Shading(s) => &s.transfer,
            Command::Group(g) => {
                collect(&g.commands, functions, limits, depth + 1)?;
                continue;
            }
        };
        if let Some(transfer) = transfer
            && !functions.contains_key(&(Arc::as_ptr(transfer) as usize))
        {
            if functions.len() >= limits.value_nodes.min(limits.decoded_bytes / 128) {
                return Err(Error::limit("transfer function selection count exceeded"));
            }
            functions.insert(Arc::as_ptr(transfer) as usize, Arc::clone(transfer));
        }
    }
    Ok(())
}

/// A path or glyph-run paint selects its own transfer and any in its tiling cell.
fn paint_transfer<'a>(
    p: &'a PathCommand,
    functions: &mut BTreeMap<usize, Transfer>,
    limits: Limits,
    depth: usize,
) -> Result<&'a Option<Transfer>> {
    if let crate::color::Color::Pattern {
        selected: Some(pattern),
        ..
    } = &p.color
        && let crate::pattern::Kind::Tiling(cell) = &pattern.kind
    {
        // Cell transfers require pattern-region selection. Discover
        // them so validate rejects the outer pattern explicitly.
        collect(&cell.commands, functions, limits, depth + 1)?;
    }
    Ok(&p.transfer)
}

impl Context {
    pub(super) fn begin_transfers(&mut self, tile: Tile) -> Result<()> {
        if let Some(regions) = &mut self.transfers {
            let before = regions.bytes();
            regions.begin(
                tile,
                self.limits
                    .decoded_bytes
                    .saturating_sub(self.allocated)
                    .saturating_add(before),
            )?;
            self.allocated += regions.bytes() - before;
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
        initial: Backdrop<'_>,
        model: Model,
        depth: usize,
    ) -> Result<()> {
        let opaque = self.transfers.as_ref().is_none_or(|r| r.opaque);
        if let Some(regions) = &mut self.transfers {
            regions.opaque &= group.composite.simple();
        }
        let result = self.commands(
            &group.commands,
            surface,
            initial,
            model,
            group.attributes.knockout,
            depth + 1,
        );
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

pub(super) fn validate(command: &Command) -> Result<()> {
    let (composite, pattern) = match command {
        Command::Path(p) => (
            &p.composite,
            matches!(p.color, crate::color::Color::Pattern { .. }),
        ),
        Command::Glyphs(run) => (
            &run.paint.composite,
            matches!(run.paint.color, crate::color::Color::Pattern { .. }),
        ),
        Command::Image(i) => (&i.composite, false),
        Command::Shading(s) => (&s.composite, false),
        Command::Group(_) => return Ok(()),
    };
    if composite.overprint || pattern {
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
        let mut regions = Regions::new(&list.commands, Limits::default())
            .unwrap()
            .unwrap();
        let tile = Tile {
            x: 7,
            y: 9,
            width: 2,
            height: 2,
        };
        let required = regions.bytes() + 4 * size_of::<usize>();
        assert_eq!(
            regions.begin(tile, required - 1).unwrap_err().kind,
            ErrorKind::Limit
        );
        regions.begin(tile, required).unwrap();
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
                .apply(0, [0.25; 3], &mut Limits::default().operations)
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
}
