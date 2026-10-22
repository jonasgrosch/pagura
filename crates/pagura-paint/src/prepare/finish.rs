//! The end of preparation: soft-mask extents, activation orders, stack
//! sizes, output, the reservations of buffers and output, the coverage
//! producer, the workspace, and the check of transfer selections. When the
//! walk failed, the transfer check of the marks before the failure decides
//! which error comes first in source order.

use super::planner::{Parts, Planner, needs};
use super::{Backend, Budget, Producer, index};
use crate::execute::{Sizes, Workspace, validate};
use crate::plan::{Item, Layout, MAX_BAND_HEIGHT, Output, Plan, SpotPlan, area, intersect};
use pagura_color::{ColorSpace, Colorant, Conversion, Process, Target};
use pagura_core::{Error, Meter, Result};
use pagura_raster::{BandCoverage, Rgba8};
use std::sync::Arc;

/// Plans the page and completes preparation.
///
/// # Errors
///
/// The first error in source order, then the reservations of buffers and
/// output.
pub(crate) fn finish<B: Backend>(
    mut planner: Planner<'_, B>,
    budget: &mut Budget,
) -> Result<Finished> {
    if let Err(error) = planner.page(budget) {
        return Err(first(planner, error));
    }
    complete(planner, budget)
}

/// Between the walk's error and a transfer boundary met by an earlier mark,
/// the one earlier in source order.
fn first<B: Backend>(planner: Planner<'_, B>, error: Error) -> Error {
    if !planner.storage.transfers {
        return error;
    }
    let serial = planner.failed.map_or(planner.serial, |(serial, _)| serial);
    let area = planner.area;
    let rows = rows(area.region.height());
    let Planner { backend, parts, .. } = planner;
    let mut scratch = Meter::new(usize::MAX, "transfer check scratch");
    let Ok(mut producer) = backend.finish(area, &mut scratch) else {
        return error;
    };
    let plan = partial(area, parts);
    let width = index(area.region.width());
    let mut coverage = BandCoverage::default();
    let mut selection = vec![0; width.saturating_mul(rows)];
    match validate(&plan, producer.as_mut(), &mut coverage, &mut selection) {
        Some(found) if found < serial => transfer_error(&plan, found),
        _ => error,
    }
}

fn transfer_error(plan: &Plan, serial: u32) -> Error {
    let position = plan
        .items
        .iter()
        .find(|item| item.serial == serial)
        .map(|item| item.position)
        .unwrap_or_default();
    Error::unsupported(
        crate::features::TRANSPARENCY_TRANSFER,
        "fractional transfer regions require correlated colour coverage",
    )
    .at(position)
    .in_object_opt(position.object())
}

/// Rows of the band buffers for a region `height` rows tall.
fn rows(height: u32) -> usize {
    index(height.min(MAX_BAND_HEIGHT))
}

/// The plan of a walk that stopped, enough for the transfer check.
fn partial(area: pagura_raster::Area, parts: Parts) -> Plan {
    Plan {
        area,
        groups: parts.groups,
        items: parts.items,
        masks: parts.masks,
        paints: parts.paints,
        conversions: parts.conversions,
        output: Output {
            layout: Layout {
                process: Process::Rgb,
                spots: 0,
            },
            background: [0; 4],
            spots: Vec::new(),
            transfers: parts.transfers,
        },
        stack: crate::execute::Tops::default(),
    }
}

/// A completed preparation.
pub(crate) struct Finished {
    pub(crate) plan: Plan,
    pub(crate) producer: Box<dyn Producer>,
    pub(crate) workspace: Workspace,
    pub(crate) repairs: Vec<pagura_core::Diagnostic>,
}

fn complete<B: Backend>(planner: Planner<'_, B>, budget: &mut Budget) -> Result<Finished> {
    let area = planner.area;
    let rows = rows(area.region.height());
    let transfers = planner.storage.transfers;
    let spots = spot_plans(&planner.definitions);
    let background = premultiplied(planner.request.background)?;
    let Planner {
        mut backend,
        mut parts,
        users,
        repairs,
        ..
    } = planner;
    for (mask, users) in parts.masks.iter_mut().zip(&users) {
        let extent = intersect(mask.extent, *users);
        mask.extent = extent;
        if let Some(group) = parts.groups.get_mut(index(mask.group)) {
            group.bounds = extent;
        }
    }
    order(&mut parts);
    let stack = needs(&parts, rows);
    let mut output_spots = Vec::new();
    for (space, inputs) in spots {
        let target = Target::new(Process::Cmyk);
        let convert = backend.converter(&space, &target, &Conversion::default())?;
        output_spots.push(SpotPlan {
            convert,
            components: space.components(),
            inputs,
        });
    }
    let layout = parts.groups.first().map_or(
        Layout {
            process: Process::Rgb,
            spots: 0,
        },
        |page| page.layout,
    );
    let mut plan = Plan {
        area,
        groups: parts.groups,
        items: parts.items,
        masks: parts.masks,
        paints: parts.paints,
        conversions: parts.conversions,
        output: Output {
            layout,
            background,
            spots: output_spots,
            transfers: parts.transfers,
        },
        stack,
    };
    shrink(&mut plan);
    reserve_output(&plan, budget)?;
    let sizes = Sizes {
        width: index(area.region.width()),
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
        selections: transfers,
    };
    budget.bytes.charge(sizes.bytes(&plan))?;
    let mut producer = backend.finish(area, &mut budget.bytes)?;
    let mut workspace = Workspace::new(&plan, sizes)?;
    if transfers {
        let checked: usize = plan
            .items
            .iter()
            .filter(|item| matches!(&item.kind, crate::plan::ItemKind::Mark(mark) if mark.records))
            .map(|item| item.work)
            .fold(0, usize::saturating_add);
        budget.work.charge(checked)?;
        if let Some(serial) = validate(
            &plan,
            producer.as_mut(),
            &mut workspace.coverage,
            &mut workspace.selection,
        ) {
            return Err(transfer_error(&plan, serial));
        }
    }
    Ok(Finished {
        plan,
        producer,
        workspace,
        repairs,
    })
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
                .map(|f| f.cost())
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
fn order(parts: &mut Parts) {
    let items = &parts.items;
    for group in &mut parts.groups {
        let first = |position: &u32| {
            group
                .items
                .get(index(*position))
                .and_then(|&id| items.get(index(id)))
                .map_or(u32::MAX, |item: &Item| item.bands[0])
        };
        let mut order: Vec<u32> =
            (0..u32::try_from(group.items.len()).unwrap_or(u32::MAX)).collect();
        order.sort_by_key(first);
        group.order = order;
    }
}

fn shrink(plan: &mut Plan) {
    plan.items.shrink_to_fit();
    plan.groups.shrink_to_fit();
    for group in &mut plan.groups {
        group.items.shrink_to_fit();
        group.order.shrink_to_fit();
    }
    plan.masks.shrink_to_fit();
    plan.paints.shrink_to_fit();
    plan.conversions.shrink_to_fit();
    plan.output.transfers.shrink_to_fit();
}

/// Bytes the plan's own structures retain.
#[cfg(test)]
pub(crate) fn plan_bytes(plan: &Plan) -> usize {
    let groups: usize = plan
        .groups
        .iter()
        .map(|g| (g.items.capacity() + g.order.capacity()) * size_of::<u32>())
        .sum();
    let paints: usize = plan
        .paints
        .iter()
        .map(|paint| size_of_val::<dyn crate::paint::PaintSpans>(&**paint))
        .sum();
    plan.items.capacity() * size_of::<Item>()
        + plan.groups.capacity() * size_of::<crate::plan::GroupPlan>()
        + groups
        + plan.masks.capacity() * size_of::<crate::plan::MaskPlan>()
        + plan.paints.capacity() * size_of::<Box<dyn crate::paint::PaintSpans>>()
        + paints
        + plan.conversions.capacity() * size_of::<crate::plan::Conversion>()
}

/// The background premultiplied once, as the output surface stores it.
fn premultiplied(background: Rgba8) -> Result<[u8; 4]> {
    let pixel = pagura_raster::Pixmap::new(1, 1, background, 1)?;
    let mut bytes = [0; 4];
    for (out, &byte) in bytes.iter_mut().zip(pixel.data()) {
        *out = byte;
    }
    Ok(bytes)
}

/// A colorant definition and the `(component, spot channel)` pairs it
/// supplies.
type Definition = (Arc<ColorSpace>, Vec<(usize, usize)>);

/// Spot channels grouped by their first painting definition (ADR 0004 §7).
fn spot_plans(definitions: &[(pagura_core::Name, Arc<ColorSpace>)]) -> Vec<Definition> {
    let mut used = vec![false; definitions.len()];
    let mut plans = Vec::new();
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
        plans.push((Arc::clone(space), inputs));
    }
    plans
}

/// Whether two colorant spaces share their tint transform.
fn same(a: &ColorSpace, b: &ColorSpace) -> bool {
    match (a, b) {
        (ColorSpace::Separation(a), ColorSpace::Separation(b)) => Arc::ptr_eq(&a.tint, &b.tint),
        (ColorSpace::DeviceN(a), ColorSpace::DeviceN(b)) => Arc::ptr_eq(&a.tint, &b.tint),
        _ => false,
    }
}
