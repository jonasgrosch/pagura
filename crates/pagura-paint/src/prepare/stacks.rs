//! The group buffer stacks a band needs: for every buffer, the largest
//! sum of nested buffers alive at once, from the innermost out.

use super::index;
use super::planner::Parts;
use crate::execute::Tops;
use crate::format::Format;
use crate::plan::ItemKind;
use pagura_core::{Meter, Result};

/// The tops of the stacks needed to run `group`, from the innermost buffer
/// out.
pub(crate) fn needs(parts: &Parts, rows: usize, bytes: &mut Meter) -> Result<Tops> {
    let mut memo = Vec::new();
    bytes.reserve_capacity(&mut memo, parts.groups.len())?;
    memo.resize(parts.groups.len(), None);
    let tops = need(parts, 0, rows, &mut memo, 0);
    let released = memo.capacity() * size_of::<Option<Tops>>();
    drop(memo);
    bytes.refund(released);
    Ok(tops)
}

fn need(parts: &Parts, group: u32, rows: usize, memo: &mut [Option<Tops>], depth: usize) -> Tops {
    if let Some(Some(done)) = memo.get(index(group)) {
        return *done;
    }
    let Some(plan) = parts.groups.get(index(group)) else {
        return Tops::default();
    };
    if depth > parts.groups.len() {
        return Tops::default();
    }
    let mut most = Tops::default();
    let mask_need = |mask: Option<u32>, memo: &mut [Option<Tops>]| {
        mask.and_then(|m| parts.masks.get(index(m)))
            .map_or(Tops::default(), |m| {
                need(parts, m.group, rows, memo, depth + 1)
            })
    };
    for &id in &plan.items {
        let Some(item) = parts.items.get(index(id)) else {
            continue;
        };
        let inner = match &item.kind {
            ItemKind::Mark(mark) => mask_need(mark.compositing.mask, memo),
            ItemKind::Run(run) => mask_need(run.mark.compositing.mask, memo),
            ItemKind::Group(used) => {
                let child = need(parts, used.group, rows, memo, depth + 1);
                let own = own(parts, used.group, rows);
                max(child, add(own, mask_need(used.compositing.mask, memo)))
            }
        };
        most = max(most, inner);
    }
    let total = if group == 0 {
        most
    } else {
        add(own(parts, group, rows), most)
    };
    if let Some(slot) = memo.get_mut(index(group)) {
        *slot = Some(total);
    }
    total
}

fn own(parts: &Parts, group: u32, rows: usize) -> Tops {
    let Some(plan) = parts.groups.get(index(group)) else {
        return Tops::default();
    };
    let pixels = pagura_core::convert::u32_to_usize(plan.bounds.width())
        .saturating_mul(pagura_core::convert::u32_to_usize(plan.bounds.height()).min(rows));
    match plan.format {
        Format::F64 => Tops {
            wide: pixels.saturating_mul(plan.layout.stride()),
            ..Tops::default()
        },
        Format::F32 | Format::N16 => Tops {
            levels: pixels,
            ..Tops::default()
        },
    }
}

fn add(a: Tops, b: Tops) -> Tops {
    Tops {
        wide: a.wide.saturating_add(b.wide),
        levels: a.levels.saturating_add(b.levels),
    }
}

fn max(a: Tops, b: Tops) -> Tops {
    Tops {
        wide: a.wide.max(b.wide),
        levels: a.levels.max(b.levels),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{GroupPlan, Layout, Start};
    use pagura_color::Process;
    use pagura_core::ErrorKind;
    use pagura_raster::DeviceRect;

    #[test]
    fn stack_analysis_reserves_then_returns_its_memo_storage() {
        let parts = Parts {
            groups: (0..2)
                .map(|_| GroupPlan {
                    bounds: DeviceRect::default(),
                    items: Vec::new(),
                    order: Vec::new(),
                    layout: Layout {
                        process: Process::Rgb,
                        spots: 0,
                    },
                    format: Format::N16,
                    knockout: false,
                    start: Start::Transparent,
                })
                .collect(),
            ..Parts::default()
        };
        let required = parts.groups.len() * size_of::<Option<Tops>>();
        let mut bytes = Meter::new(31 + required - 1, "stack analysis memory");
        bytes.charge(31).unwrap();
        let error = needs(&parts, 16, &mut bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "stack analysis memory")
        );
        assert_eq!(bytes.used(), 31);

        let mut bytes = Meter::new(31 + required, "stack analysis memory");
        bytes.charge(31).unwrap();
        let result = needs(&parts, 16, &mut bytes).unwrap();
        assert_eq!((result.wide, result.levels), (0, 0));
        assert_eq!(bytes.used(), 31);
    }
}
