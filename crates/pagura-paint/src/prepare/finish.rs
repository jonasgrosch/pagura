//! The end of preparation: soft-mask extents, activation orders, stack
//! sizes, output, the reservations of buffers and output, the prepared
//! shapes, the workspace's sizes, and the check of transfer selections.
//! When the walk failed, the transfer check of the marks before the failure
//! decides which error comes first in source order.

use super::planner::{Parts, Planner};
use super::stacks::needs;
use super::{Budget, Shapes, index};
use crate::execute::{Sizes, coverage_capacity, validate};
use crate::plan::{Curve, Item, Layout, MAX_BAND_HEIGHT, Output, Plan, SpotPlan, area, intersect};
use pagura_color::{ColorSpace, Colorant, Conversion, Converter, Process, Target};
use pagura_core::{Error, Meter, Result};
use pagura_raster::{BandCoverage, BandRasterizer, Rgba8};
use std::sync::Arc;

/// Plans the page and completes preparation.
///
/// # Errors
///
/// The first error in source order, then the reservations of buffers and
/// output.
pub(crate) fn finish(mut planner: Planner<'_>, budget: &mut Budget) -> Result<Finished> {
    if let Err(error) = planner.page(budget) {
        return Err(first(planner, error));
    }
    complete(planner, budget)
}

/// Between the walk's error and a transfer boundary met by an earlier mark,
/// the one earlier in source order.
fn first(planner: Planner<'_>, error: Error) -> Error {
    if !planner.storage.transfers {
        return error;
    }
    let serial = planner.failed.map_or(planner.serial, |(serial, _)| serial);
    let area = planner.area;
    let limit = planner.limits.render_bytes;
    let Planner { shapes, parts, .. } = planner;
    let shapes = shapes.finish();
    let plan = partial(area, parts);
    // Preparation fails either way, so the check's storage is bounded by
    // the budget rather than reserved.
    let mut bytes = Meter::new(limit, "render memory budget exceeded");
    match check_transfers(&plan, &shapes, &mut bytes) {
        Ok(Some(found)) if found < serial => transfer_error(&plan, found),
        _ => error,
    }
}

/// The serial of the first recording mark whose transfer selection is not
/// whole, rasterized in storage of its own charged to `bytes`: a band
/// rasterizer, one mark's coverage and a band of selections, no more than a
/// render's scratch holds of them.
fn check_transfers(plan: &Plan, shapes: &Shapes, bytes: &mut Meter) -> Result<Option<u32>> {
    let area = plan.area;
    let rows = index(area.band_height.min(area.region.height()));
    let width = index(area.region.width());
    let mut rasterizer = BandRasterizer::new(area, bytes)?;
    let mut coverage = BandCoverage::with_capacity(coverage_capacity(width, rows), bytes)?;
    let mut selection = Vec::new();
    bytes.reserve(&mut selection, width.saturating_mul(rows))?;
    selection.resize(width.saturating_mul(rows), 0);
    Ok(validate(
        plan,
        (shapes, &mut rasterizer),
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
        scratch: 0,
    }
}

/// A completed preparation.
pub(crate) struct Finished {
    pub(crate) plan: Plan,
    pub(crate) shapes: Shapes,
    pub(crate) repairs: Vec<pagura_core::Diagnostic>,
}

fn complete(planner: Planner<'_>, budget: &mut Budget) -> Result<Finished> {
    let area = planner.area;
    // Buffers are charged for the tallest band any band height gives, so
    // that charges never depend on it, and hold the tallest band of this
    // one.
    let rows = rows(area.region.height());
    let band_rows = index(area.band_height.min(area.region.height()));
    let transfers = planner.storage.transfers;
    let spots = spot_plans(&planner.definitions, &mut budget.bytes)?;
    let background = premultiplied(planner.request.background)?;
    let Planner {
        shapes,
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
        area,
        groups: parts.groups,
        items: parts.items,
        masks: parts.masks,
        paints: parts.paints,
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
    shrink(&mut plan);
    reserve_output(&plan, budget)?;
    let sizes = sizes(&plan, rows, transfers);
    let workspace = sizes.bytes(&plan, charged);
    budget.bytes.charge(workspace)?;
    let before = budget.bytes.used();
    BandRasterizer::charge(area, &mut budget.bytes)?;
    plan.scratch = workspace.saturating_add(budget.bytes.used() - before);
    plan.sizes = Sizes {
        rows: band_rows,
        ..sizes
    };
    let shapes = shapes.finish();
    if transfers {
        let checked: usize = plan
            .items
            .iter()
            .filter(|item| matches!(&item.kind, crate::plan::ItemKind::Mark(mark) if mark.records))
            .map(|item| item.work)
            .fold(0, usize::saturating_add);
        budget.work.charge(checked)?;
        // The check runs before any render, in storage its reservation
        // covers.
        let mut bytes = Meter::new(plan.scratch, "render memory budget exceeded");
        if let Some(serial) = check_transfers(&plan, &shapes, &mut bytes)? {
            return Err(transfer_error(&plan, serial));
        }
    }
    Ok(Finished {
        plan,
        shapes,
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
        // Each item's two u32 list slots were reserved by ITEM_BYTES.
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

/// The background premultiplied once, as the output surface stores it.
fn premultiplied(background: Rgba8) -> Result<[u8; 4]> {
    let pixel = pagura_raster::Pixmap::new(1, 1, background, 1)?;
    let mut bytes = [0; 4];
    for (out, &byte) in bytes.iter_mut().zip(pixel.data()) {
        *out = byte;
    }
    Ok(bytes)
}

/// Spot channels grouped by their first painting definition (ADR 0004 §7).
fn spot_plans(
    definitions: &[(pagura_core::Name, Arc<ColorSpace>)],
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
        let target = Target::new(Process::Cmyk);
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
mod tests {
    use super::*;
    use pagura_color::{CalculatorFunction, DeviceN, Function, Indexed};
    use pagura_core::{Limits, Name};

    #[test]
    fn activation_order_keeps_source_positions_with_equal_first_bands() {
        use crate::format::Format;
        use crate::plan::{Compositing, GroupPlan, GroupUse, ItemKind, Start};
        use pagura_interpret::BlendMode;
        use pagura_raster::DeviceRect;

        let mut parts = Parts {
            groups: vec![GroupPlan {
                bounds: DeviceRect::default(),
                items: vec![4, 0, 3, 1, 2],
                order: Vec::new(),
                layout: Layout {
                    process: Process::Rgb,
                    spots: 0,
                },
                format: Format::N16,
                knockout: false,
                start: Start::Transparent,
            }],
            items: [2, 0, 2, 0, 0]
                .map(|first| Item {
                    bounds: DeviceRect::default(),
                    bands: [first, first],
                    serial: 0,
                    position: pagura_core::Position::default(),
                    work: 0,
                    kind: ItemKind::Group(GroupUse {
                        group: 0,
                        cover: None,
                        compositing: Compositing {
                            alpha: 1.0,
                            blend: BlendMode::Normal,
                            alpha_is_shape: false,
                            mask: None,
                            overprint: 0,
                        },
                        conversion: None,
                    }),
                })
                .into(),
            ..Parts::default()
        };
        order(&mut parts).unwrap();
        assert_eq!(parts.groups[0].items, [4, 0, 3, 1, 2]);
        assert_eq!(parts.groups[0].order, [0, 2, 3, 1, 4]);
        let capacity = parts.groups[0].order.capacity();
        order(&mut parts).unwrap();
        assert_eq!(parts.groups[0].order, [0, 2, 3, 1, 4]);
        assert_eq!(parts.groups[0].order.capacity(), capacity);
    }

    #[test]
    fn spot_plans_share_their_definition_and_charge_live_conversion_storage() {
        let names = [Name::new(b"First"), Name::new(b"Second")];
        let tint = Function::calculator(
            CalculatorFunction {
                domain: vec![[0.0, 1.0]; 2],
                range: vec![[0.0, 1.0]],
                program: Arc::from(b"{ add 0.5 mul }".as_slice()),
            },
            &Limits::default(),
        )
        .unwrap();
        let space = Arc::new(ColorSpace::DeviceN(Arc::new(DeviceN {
            colorants: names.iter().cloned().map(Colorant::Named).collect(),
            alternate: Arc::new(ColorSpace::Indexed(Arc::new(Indexed {
                base: Arc::new(ColorSpace::DeviceRgb),
                high: 255,
                lookup: vec![128; 256 * 3].into(),
            }))),
            tint: Arc::new(tint),
        })));
        let definitions = names.map(|name| (name, Arc::clone(&space)));
        let mut bytes = Meter::new(1 << 20, "spot memory");
        bytes.charge(31).unwrap();
        let plans = spot_plans(&definitions, &mut bytes).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].inputs, [(0, 0), (1, 1)]);
        let retained = plans.capacity() * size_of::<SpotPlan>()
            + plans[0].inputs.capacity() * size_of::<(usize, usize)>()
            + plans[0].convert.owned_bytes();
        assert_eq!(bytes.used(), 31 + retained);
        let mut tiny = Meter::new(256 * 3 * size_of::<f64>() - 1, "spot memory");
        assert_eq!(
            spot_plans(&definitions, &mut tiny).unwrap_err().message,
            "spot memory"
        );
    }
}
