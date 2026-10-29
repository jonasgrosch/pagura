//! The end of preparation: soft-mask extents, activation orders, stack
//! sizes, output, the reservations of buffers and output, the prepared
//! shapes, the workspace's sizes, and the check of transfer selections.
//! When the walk failed, the transfer check of the marks before the failure
//! decides which error comes first in source order.

use super::planner::{Parts, Planner, Walked};
use super::stacks::needs;
use super::{Budget, Shapes, index};
use crate::execute::{Sizes, coverage_capacity, validate};
use crate::plan::{Curve, Item, Layout, MAX_BAND_HEIGHT, Output, Plan, SpotPlan, area, intersect};
use pagura_color::{ColorSpace, Colorant, Conversion, Converter, Process, Target};
use pagura_core::{Error, Meter, Result};
use pagura_raster::{BandCoverage, BandRasterizer};
use std::sync::Arc;

/// Plans the page and completes preparation.
///
/// # Errors
///
/// The first error in source order, then the reservations of buffers and
/// output.
pub(crate) fn finish(mut planner: Planner<'_>, budget: &mut Budget) -> Result<Finished> {
    if let Err(error) = planner.page(budget) {
        return Err(first(planner, error, budget));
    }
    complete(planner, budget)
}

/// Between the walk's error and a transfer boundary met by an earlier mark,
/// the one earlier in source order.
fn first(planner: Planner<'_>, error: Error, budget: &mut Budget) -> Error {
    if !planner.storage.transfers {
        return error;
    }
    let serial = planner.failed.map_or(planner.serial, |(serial, _)| serial);
    let area = planner.area;
    let Ok(Walked {
        shapes,
        parts,
        definitions,
        transfer_bytes,
        ..
    }) = planner.finish(&mut budget.bytes)
    else {
        return error;
    };
    drop_definitions(definitions, &mut budget.bytes);
    let Ok(shapes) = shapes.finish(&mut budget.bytes) else {
        return error;
    };
    let checked = held_transfer_check(area, &parts, &shapes, transfer_bytes, &mut budget.bytes);
    match checked {
        Ok(Some(found)) if found < serial => transfer_error(&parts.items, found),
        _ => error,
    }
}

/// Reserves the fixed worst-band checker before the ordered walk, so a
/// later mark's allocations cannot hide an earlier transfer boundary.
pub(super) fn reserve_transfer_check(
    area: pagura_raster::Area,
    bytes: &mut Meter,
) -> Result<usize> {
    let mut reservation = Meter::new(bytes.remaining(), "render memory budget exceeded");
    BandRasterizer::charge(area, &mut reservation)?;
    let width = index(area.region.width());
    let rows = rows(area.region.height());
    reservation.charge(coverage_capacity(width, rows).bytes())?;
    reservation.charge_product(width, rows.saturating_mul(size_of::<u16>()))?;
    let reserved = reservation.used();
    bytes.charge(reserved)?;
    Ok(reserved)
}

/// Uses the already held reservation; every checker allocation is dropped
/// before that reservation returns to finalization's shared allowance.
fn held_transfer_check(
    area: pagura_raster::Area,
    parts: &Parts,
    shapes: &Shapes,
    reserved: usize,
    bytes: &mut Meter,
) -> Result<Option<u32>> {
    if reserved == 0 {
        return Ok(None);
    }
    let checked = check_transfers(
        area,
        parts,
        shapes,
        &mut Meter::new(reserved, "render memory budget exceeded"),
    );
    bytes.refund(reserved);
    checked
}

/// The serial of the first recording mark whose transfer selection is not
/// whole, rasterized in storage of its own charged to `bytes`: a band
/// rasterizer, one mark's coverage and a band of selections, no more than a
/// render's scratch holds of them.
fn check_transfers(
    area: pagura_raster::Area,
    parts: &Parts,
    shapes: &Shapes,
    bytes: &mut Meter,
) -> Result<Option<u32>> {
    let capacity_rows = rows(area.region.height());
    let actual_rows = index(area.band_height.min(area.region.height()));
    let width = index(area.region.width());
    let mut rasterizer = BandRasterizer::new(area, bytes)?;
    let mut coverage = BandCoverage::with_capacity(coverage_capacity(width, capacity_rows), bytes)?;
    let mut selection = Vec::new();
    // Only the allocation uses the fixed worst-band bound. Validation
    // still visits the prepared grid and clears its actual requested rows.
    bytes.reserve(&mut selection, width.saturating_mul(capacity_rows))?;
    selection.resize(width.saturating_mul(actual_rows), 0);
    Ok(validate(
        area,
        (&parts.groups, &parts.items),
        (shapes, &mut rasterizer),
        &mut coverage,
        &mut selection,
    ))
}

fn transfer_error(items: &[Item], serial: u32) -> Error {
    let position = items
        .iter()
        .find(|item| item.serial == serial)
        .map(|item| item.position)
        .unwrap_or_default();
    Error::unsupported(
        pagura_interpret::features::TRANSPARENCY_TRANSFER,
        "fractional transfer regions require correlated colour coverage",
    )
    .at(position)
    .in_object_opt(position.object())
}

/// Rows of the band buffers for a region `height` rows tall.
fn rows(height: u32) -> usize {
    index(height.min(MAX_BAND_HEIGHT))
}

/// A completed preparation.
pub(crate) struct Finished {
    pub(crate) plan: Plan,
    pub(crate) shapes: Shapes,
    pub(crate) repairs: Vec<pagura_core::Diagnostic>,
}

fn complete(mut planner: Planner<'_>, budget: &mut Budget) -> Result<Finished> {
    let area = planner.area;
    // Buffers are charged for the tallest band any band height gives, so
    // that charges never depend on it, and hold the tallest band of this
    // one.
    let rows = rows(area.region.height());
    let band_rows = index(area.band_height.min(area.region.height()));
    let transfers = planner.storage.transfers;
    let background = planner.request.background.premultiplied();
    for (mask, users) in planner.parts.masks.iter_mut().zip(&planner.users) {
        let extent = intersect(mask.extent, *users);
        mask.extent = extent;
        if let Some(group) = planner.parts.groups.get_mut(index(mask.group)) {
            group.bounds = extent;
        }
    }
    let Walked {
        shapes,
        names,
        mut parts,
        repairs,
        definitions,
        transfer_bytes,
    } = planner.finish(&mut budget.bytes)?;
    let shapes = shapes.finish(&mut budget.bytes)?;
    if let Some(serial) =
        held_transfer_check(area, &parts, &shapes, transfer_bytes, &mut budget.bytes)?
    {
        return Err(transfer_error(&parts.items, serial));
    }
    let spots = spot_plans(&definitions, &names.empty, &mut budget.bytes);
    drop_definitions(definitions, &mut budget.bytes);
    let spots = spots?;
    order(&mut parts)?;
    let charged = needs(&parts, rows, &mut budget.bytes)?;
    let stack = needs(&parts, band_rows, &mut budget.bytes)?;
    let layout = parts.groups.first().map_or(
        Layout {
            process: Process::Rgb,
            spots: 0,
        },
        |page| page.layout,
    );
    let mut plan = Plan {
        _names: names,
        area,
        groups: parts.groups,
        items: parts.items,
        masks: parts.masks,
        paints: parts.paints,
        slots: parts.slots,
        conversions: parts.conversions,
        output: Output {
            layout,
            background,
            spots,
            transfers: parts.transfers,
        },
        stack,
        sizes: Sizes::default(),
        scratch: 0,
    };
    reserve_output(&plan, budget)?;
    let sizes = sizes(&plan, rows, transfers);
    let workspace = sizes.bytes(&plan, charged);
    budget.bytes.charge(workspace)?;
    let before = budget.bytes.used();
    BandRasterizer::charge(area, &mut budget.bytes)?;
    plan.scratch = workspace.saturating_add(budget.bytes.used() - before);
    // Optional storage comes last, from what the page's own storage leaves:
    // it never refuses a page that admits without it.
    for paint in &mut plan.paints {
        paint.admit_optional(&mut budget.bytes);
    }
    plan.sizes = Sizes {
        rows: band_rows,
        ..sizes
    };
    Ok(Finished {
        plan,
        shapes,
        repairs,
    })
}

fn drop_definitions(definitions: Vec<(pagura_core::Name, Arc<ColorSpace>)>, bytes: &mut Meter) {
    let released = definitions.capacity() * size_of::<(pagura_core::Name, Arc<ColorSpace>)>()
        + definitions
            .iter()
            .map(|(name, _)| name.heap_bytes())
            .sum::<usize>();
    drop(definitions);
    bytes.refund(released);
}

/// The sizes of the workspace of `plan` for bands of `rows` rows.
fn sizes(plan: &Plan, rows: usize, selections: bool) -> Sizes {
    Sizes {
        width: index(plan.area.region.width()),
        rows,
        channels: plan
            .groups
            .iter()
            .map(|g| g.layout.channels())
            .max()
            .unwrap_or(3)
            .max(4),
        stride: plan
            .groups
            .iter()
            .map(|g| g.layout.stride())
            .max()
            .unwrap_or(6)
            .max(7),
        selections,
    }
}

/// The output's work and function evaluations: one unit per pixel, and per
/// pixel the spot equivalents and one selected transfer function per RGB
/// channel at most.
fn reserve_output(plan: &Plan, budget: &mut Budget) -> Result<()> {
    let pixels = area(plan.area.region);
    budget.work.charge(pixels)?;
    let spots = plan
        .output
        .spots
        .iter()
        .map(|spot| spot.convert.cost())
        .fold(0, usize::saturating_add);
    let transfers = plan
        .output
        .transfers
        .iter()
        .map(|functions| {
            functions
                .iter()
                .flatten()
                .map(Curve::cost)
                .fold(0, usize::saturating_add)
        })
        .max()
        .unwrap_or(0);
    budget
        .functions
        .charge_product(pixels, spots.saturating_add(transfers))
}

/// Each group's activation order: positions by first band, then source
/// order.
fn order(parts: &mut Parts) -> Result<()> {
    let items = &parts.items;
    let first = |id: u32| {
        items
            .get(index(id))
            .map_or(u32::MAX, |item: &Item| item.bands[0])
    };
    for group in &mut parts.groups {
        // The ordered walk reserved each future order slot separately
        // from the capacity of the source-order item list.
        // Order in place; the source position makes ties explicit, without
        // allocating the scratch of a stable sort.
        group.order.clear();
        group
            .order
            .try_reserve_exact(group.items.len())
            .map_err(|_| Error::limit("render memory budget exceeded"))?;
        group
            .order
            .extend((0..group.items.len()).map(|i| u32::try_from(i).unwrap_or(u32::MAX)));
        group.order.sort_unstable_by_key(|&position| {
            (
                group
                    .items
                    .get(index(position))
                    .map_or(u32::MAX, |&id| first(id)),
                position,
            )
        });
    }
    Ok(())
}

/// Spot channels grouped by their first painting definition (ADR 0004 §7).
fn spot_plans(
    definitions: &[(pagura_core::Name, Arc<ColorSpace>)],
    empty: &Arc<[pagura_core::Name]>,
    bytes: &mut Meter,
) -> Result<Vec<SpotPlan>> {
    let mut used = Vec::new();
    bytes.reserve(&mut used, definitions.len())?;
    used.resize(definitions.len(), false);
    let mut plans = Vec::new();
    bytes.reserve(&mut plans, definitions.len())?;
    for (spot, (_, space)) in definitions.iter().enumerate() {
        if used.get(spot).copied().unwrap_or(true) {
            continue;
        }
        let colorants: &[Colorant] = match &**space {
            ColorSpace::Separation(separation) => std::slice::from_ref(&separation.colorant),
            ColorSpace::DeviceN(device) => &device.colorants,
            _ => continue,
        };
        let mut inputs = Vec::new();
        bytes.reserve(&mut inputs, colorants.len())?;
        for (component, colorant) in colorants.iter().enumerate() {
            let Colorant::Named(name) = colorant else {
                continue;
            };
            let found = definitions
                .iter()
                .enumerate()
                .position(|(_, (other, definition))| other == name && same(definition, space));
            if let Some(channel) = found {
                if let Some(flag) = used.get_mut(channel) {
                    *flag = true;
                }
                inputs.push((component, channel));
            }
        }
        let target = Target {
            process: Process::Cmyk,
            spots: Arc::clone(empty),
            colorants: pagura_color::Colorants::Composite,
        };
        let convert = Converter::new_metered(space, &target, &Conversion::default(), bytes)?;
        plans.push(SpotPlan {
            convert,
            components: space.components(),
            inputs,
        });
    }
    let released = used.capacity() * size_of::<bool>();
    drop(used);
    bytes.refund(released);
    Ok(plans)
}

/// Whether two colorant spaces share their tint transform.
fn same(a: &ColorSpace, b: &ColorSpace) -> bool {
    match (a, b) {
        (ColorSpace::Separation(a), ColorSpace::Separation(b)) => Arc::ptr_eq(&a.tint, &b.tint),
        (ColorSpace::DeviceN(a), ColorSpace::DeviceN(b)) => Arc::ptr_eq(&a.tint, &b.tint),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
