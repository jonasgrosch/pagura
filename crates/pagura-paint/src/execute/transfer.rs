//! Transfer selection checked before execution (11.7.5.2): the last opaque
//! elementary mark that fully covers a pixel selects its transfer function.
//! Where a mark covers a pixel partially and either it or the selection so
//! far is not the default, colours of differently transferred areas would be
//! averaged before the nonlinear function; that is not supported, and the
//! first such mark in source order fails preparation, so execution cannot.

use crate::plan::{Cover, GroupPlan, Item, ItemKind};
use crate::prepare::Shapes;
use pagura_core::convert::u32_to_usize;
use pagura_raster::{Area, BandCoverage, BandRasterizer, Span};

/// The serial of the first mark in source order whose coverage meets an
/// unsupported transfer boundary in the region, using `coverage` and
/// `selection` (one band of the region) as storage.
pub(crate) fn validate(
    area: Area,
    (groups, items): (&[GroupPlan], &[Item]),
    (shapes, rasterizer): (&Shapes, &mut BandRasterizer),
    coverage: &mut BandCoverage,
    selection: &mut [u16],
) -> Option<u32> {
    let mut first = None;
    for band in area.bands() {
        let rows = area.band_rows(band);
        selection.fill(0);
        let mut check = Check {
            area,
            groups,
            items,
            shapes,
            rasterizer: &mut *rasterizer,
            coverage: &mut *coverage,
            selection: &mut *selection,
            band,
            first_row: rows.start,
            rows_end: rows.end,
            found: &mut first,
        };
        check.group(0);
    }
    first
}

struct Check<'a> {
    area: Area,
    groups: &'a [GroupPlan],
    items: &'a [Item],
    shapes: &'a Shapes,
    rasterizer: &'a mut BandRasterizer,
    coverage: &'a mut BandCoverage,
    selection: &'a mut [u16],
    band: u32,
    first_row: u32,
    rows_end: u32,
    found: &'a mut Option<u32>,
}

impl Check<'_> {
    fn group(&mut self, group: u32) {
        let Some(group) = self.groups.get(u32_to_usize(group)) else {
            return;
        };
        for &id in &group.items {
            let Some(item) = self.items.get(u32_to_usize(id)) else {
                continue;
            };
            if item.bands[0] > self.band || item.bands[1] < self.band {
                continue;
            }
            match &item.kind {
                ItemKind::Mark(mark)
                    if mark.records && self.found.is_none_or(|serial| item.serial < serial) =>
                {
                    self.mark(mark.cover, mark.select, item.serial);
                }
                ItemKind::Group(used) if used.cover.is_none() => self.group(used.group),
                ItemKind::Mark(_) | ItemKind::Group(_) | ItemKind::Run(_) => {}
            }
        }
    }

    fn mark(&mut self, cover: Cover, select: u16, serial: u32) {
        let region = self.area.region;
        let width = u32_to_usize(region.width());
        let coverage = &mut *self.coverage;
        self.shapes
            .rasterize(self.rasterizer, cover, self.band, coverage);
        let mut failed = false;
        for row in coverage.rows() {
            if row.y < self.first_row || row.y >= self.rows_end {
                continue;
            }
            let base = u32_to_usize(row.y - self.first_row) * width;
            for span in row.spans() {
                let (x, len, partial) = match span {
                    Span::Full { x, len } => (x, u32_to_usize(len), None),
                    Span::Partial { x, coverage } => (x, coverage.len(), Some(coverage)),
                };
                for k in 0..len {
                    let column = x.saturating_add(u32::try_from(k).unwrap_or(u32::MAX));
                    if column < region.x0 || column >= region.x1 {
                        continue;
                    }
                    let Some(slot) = self
                        .selection
                        .get_mut(base + u32_to_usize(column - region.x0))
                    else {
                        continue;
                    };
                    let c = partial.map_or(255, |p| p.get(k).copied().unwrap_or(0));
                    if c == 255 {
                        *slot = select;
                    } else if c > 0 && (select != 0 || *slot != 0) {
                        failed = true;
                    }
                }
            }
        }
        if failed {
            *self.found = Some(self.found.map_or(serial, |found| found.min(serial)));
        }
    }
}
