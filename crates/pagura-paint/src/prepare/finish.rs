//! The end of preparation: soft-mask extents, activation orders, stack
//! sizes, output, the reservations of buffers and output, the coverage
//! producer, the workspace's sizes, and the check of transfer selections.
//! When the walk failed, the transfer check of the marks before the failure
//! decides which error comes first in source order.

use super::planner::{Parts, Planner, needs};
use super::{Backend, Budget, Producer, index};
use crate::execute::{Sizes, validate};
use crate::plan::{Item, Layout, MAX_BAND_HEIGHT, Output, Plan, SpotPlan, area, intersect};
use pagura_color::{ColorSpace, Colorant, Conversion, Process, Target};
use pagura_core::{Error, Meter, Result};
use pagura_raster::{BandCoverage, BandRasterizer, Rgba8};
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
    let Planner { backend, parts, .. } = planner;
    let producer = backend.finish(area);
    let plan = partial(area, parts);
    match check_transfers(&plan, producer.as_ref()) {
        Ok(Some(found)) if found < serial => transfer_error(&plan, found),
        _ => error,
    }
}

/// The serial of the first recording mark whose transfer selection is not
/// whole, rasterized in scratch storage of its own.
fn check_transfers(plan: &Plan, producer: &dyn Producer) -> Result<Option<u32>> {
    let area = plan.area;
    let rows = index(area.band_height.min(area.region.height()));
    // Charged with the render's rasterizer and workspace.
    let mut free = Meter::new(usize::MAX, "transfer check scratch");
    let mut rasterizer = BandRasterizer::new(area, &mut free)?;
    let mut coverage = BandCoverage::default();
    let mut selection = vec![0; index(area.region.width()).saturating_mul(rows)];
    Ok(validate(
        plan,
        (producer, &mut rasterizer),
        &mut coverage,
        &mut selection,
    ))
}

fn transfer_error(plan: &Plan, serial: u32) -> Error {
    let position = plan
        .items
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
        sizes: Sizes::default(),
    }
}

/// A completed preparation.
pub(crate) struct Finished {
    pub(crate) plan: Plan,
    pub(crate) producer: Box<dyn Producer>,
    pub(crate) repairs: Vec<pagura_core::Diagnostic>,
}

fn complete<B: Backend>(planner: Planner<'_, B>, budget: &mut Budget) -> Result<Finished> {
    let area = planner.area;
    // Buffers are charged for the tallest band any band height gives, so
    // that charges never depend on it, and hold the tallest band of this
    // one.
    let rows = rows(area.region.height());
    let band_rows = index(area.band_height.min(area.region.height()));
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
    let charged = needs(&parts, rows);
    let stack = needs(&parts, band_rows);
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
        sizes: Sizes::default(),
    };
    shrink(&mut plan);
    reserve_output(&plan, budget)?;
    let sizes = sizes(&plan, rows, transfers);
    budget.bytes.charge(sizes.bytes(&plan, charged))?;
    BandRasterizer::charge(area, &mut budget.bytes)?;
    plan.sizes = Sizes {
        rows: band_rows,
        ..sizes
    };
    let producer = backend.finish(area);
    if transfers {
        let checked: usize = plan
            .items
            .iter()
            .filter(|item| matches!(&item.kind, crate::plan::ItemKind::Mark(mark) if mark.records))
            .map(|item| item.work)
            .fold(0, usize::saturating_add);
        budget.work.charge(checked)?;
        if let Some(serial) = check_transfers(&plan, producer.as_ref())? {
            return Err(transfer_error(&plan, serial));
        }
    }
    Ok(Finished {
        plan,
        producer,
        repairs,
    })
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
        let firsts: Vec<u32> = group
            .items
            .iter()
            .map(|&id| {
                items
                    .get(index(id))
                    .map_or(u32::MAX, |item: &Item| item.bands[0])
            })
            .collect();
        group.order = by_band(&firsts);
    }
}

/// Positions `0..firsts.len()` ordered by their first band, stably: by
/// counting when the bands span a range comparable to the count, as a
/// region's bands do, by sorting otherwise.
fn by_band(firsts: &[u32]) -> Vec<u32> {
    let position = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    let (Some(&lo), Some(&hi)) = (firsts.iter().min(), firsts.iter().max()) else {
        return Vec::new();
    };
    let span = index(hi - lo).saturating_add(1);
    if span > firsts.len().saturating_mul(4).saturating_add(64) {
        let mut order: Vec<u32> = (0..firsts.len()).map(position).collect();
        order.sort_by_key(|&i| firsts.get(index(i)).copied().unwrap_or(u32::MAX));
        return order;
    }
    // Where each band's positions start, then each position in its place.
    let mut starts = vec![0_usize; span];
    for &first in firsts {
        if let Some(count) = starts.get_mut(index(first - lo)) {
            *count += 1;
        }
    }
    let mut total = 0;
    for start in &mut starts {
        let count = *start;
        *start = total;
        total += count;
    }
    let mut order = vec![0_u32; firsts.len()];
    for (i, &first) in firsts.iter().enumerate() {
        if let Some(start) = starts.get_mut(index(first - lo))
            && let Some(slot) = order.get_mut(*start)
        {
            *slot = position(i);
            *start += 1;
        }
    }
    order
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

#[cfg(test)]
mod tests {
    use super::by_band;

    #[test]
    fn positions_order_stably_by_first_band() {
        let firsts = [3, 1, 3, 0, 1, u32::MAX, 2];
        let mut expected: Vec<u32> = (0..7).collect();
        expected.sort_by_key(|&i| firsts[i as usize]);
        assert_eq!(by_band(&firsts), expected);
        let dense: Vec<u32> = (0..500).map(|i| (i * 7919) % 13).collect();
        let mut expected: Vec<u32> = (0..500).collect();
        expected.sort_by_key(|&i| dense[i as usize]);
        assert_eq!(by_band(&dense), expected);
        assert_eq!(by_band(&[]), Vec::<u32>::new());
    }
}
