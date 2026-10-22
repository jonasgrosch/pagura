//! Glyph runs (ADR 0004 §4): the placements of one text-showing operator
//! painted with one solid paint become one item. Each placement is still
//! prepared, culled and charged as a mark of its own; the run keeps their
//! shapes in content order and the rectangles where two of them may cover
//! the same pixel, so that execution can gather the coverage of pixels only
//! one placement covers into one band row and store it at once,
//! byte-identical to one item per placement.

use super::{Planner, Scope};
use crate::format::Format;
use crate::plan::{Glyph, Item, ItemKind, Mark, Run, area, intersect, union};
use crate::prepare::{Backend, Budget};
use pagura_core::{Error, Position, Result};
use pagura_interpret::{GlyphsNode, Paint};
use pagura_raster::DeviceRect;

/// The most placements of one run item, which bounds the placements a band
/// looks at per run.
const RUN_GLYPHS: usize = 256;

/// Meeting pairs per placement beyond which a run stores placement by
/// placement instead of gathering.
const MEETINGS: usize = 4;

/// A run grows while its bounds stay within this many times the pixels of
/// its placements, plus `SLACK`, so that gathering composites few pixels no
/// placement covers.
const SPREAD: usize = 4;

/// See [`SPREAD`].
const SLACK: usize = 4096;

/// Bytes charged per gathered placement.
const GLYPH_BYTES: usize = size_of::<Glyph>();

#[cfg(test)]
thread_local! {
    /// Whether glyph runs gather into run items; tests turn it off to
    /// compare with one item per placement.
    pub(crate) static RUNS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// The run being gathered.
#[derive(Debug)]
pub(crate) struct Open {
    group: u32,
    mark: Mark,
    serial: u32,
    pub(crate) position: Position,
    glyphs: Vec<Glyph>,
    bounds: DeviceRect,
    /// Pixels of the placements' bounds, summed.
    covered: usize,
    work: usize,
}

/// One prepared placement.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placed {
    pub(crate) shape: u32,
    pub(crate) bounds: DeviceRect,
    pub(crate) work: usize,
    pub(crate) serial: u32,
    pub(crate) position: Position,
}

impl<B: Backend> Planner<'_, B> {
    /// Whether `node`'s placements gather into runs: a solid paint and no
    /// transfer selection to record.
    pub(super) fn gathers(&self, scope: &Scope, node: &GlyphsNode) -> bool {
        #[cfg(test)]
        if !RUNS.with(std::cell::Cell::get) {
            return false;
        }
        if scope.records {
            return false;
        }
        let paint = match &scope.paint {
            Some((_, paint)) => Some(&**paint),
            None => self.scene.paints.get(node.paint.index()).map(|p| &**p),
        };
        matches!(paint, Some(Paint::Solid(_)))
    }

    /// Adds a placement to the open run, or starts a new one where it does
    /// not fit.
    pub(super) fn gather(
        &mut self,
        group: u32,
        mark: Mark,
        placed: Placed,
        budget: &mut Budget,
    ) -> Result<()> {
        if self.parts.items.len().saturating_add(self.gathered) >= self.limits.display_commands {
            return Err(Error::limit("painted mark budget exceeded"));
        }
        budget.bytes.charge(GLYPH_BYTES)?;
        self.gathered += 1;
        let fits = self.run.as_ref().is_some_and(|open| {
            open.group == group
                && same(&open.mark, &mark)
                && open.glyphs.len() < RUN_GLYPHS
                && area(union(open.bounds, placed.bounds))
                    <= SPREAD
                        .saturating_mul(open.covered.saturating_add(area(placed.bounds)))
                        .saturating_add(SLACK)
        });
        if !fits {
            self.flush(budget)?;
        }
        let bands = self.bands(placed.bounds);
        let open = self.run.get_or_insert_with(|| Open {
            group,
            mark,
            serial: placed.serial,
            position: placed.position,
            glyphs: Vec::new(),
            bounds: DeviceRect::default(),
            covered: 0,
            work: 0,
        });
        open.glyphs.push(Glyph {
            shape: placed.shape,
            bounds: placed.bounds,
            bands,
            shared: [0, 0],
        });
        open.bounds = union(open.bounds, placed.bounds);
        open.covered = open.covered.saturating_add(area(placed.bounds));
        open.work = open.work.saturating_add(placed.work);
        Ok(())
    }

    /// Adds the open run to its group: a mark item for one placement, a
    /// run item for more.
    pub(crate) fn flush(&mut self, budget: &mut Budget) -> Result<()> {
        let Some(open) = self.run.take() else {
            return Ok(());
        };
        let Open {
            group,
            mut mark,
            serial,
            position,
            mut glyphs,
            bounds,
            covered: _,
            work,
        } = open;
        if let [glyph] = glyphs[..] {
            mark.cover.shape = Some(glyph.shape);
            let item = Item {
                bounds: glyph.bounds,
                bands: glyph.bands,
                serial,
                position,
                work,
                kind: ItemKind::Mark(mark),
            };
            return self.push(group, item, budget);
        }
        // Opaque constant stores into the N16 or byte page need no
        // gathering: each placement stores straight from its coverage.
        let direct = group == 0
            && matches!(self.storage.page, Format::N16 | Format::U8)
            && mark.words.is_some();
        let shared = if direct { None } else { meetings(&mut glyphs) };
        let gather = shared.is_some();
        let shared = shared.unwrap_or_default();
        budget
            .bytes
            .charge_product(shared.len(), size_of::<DeviceRect>())?;
        if gather {
            // Gathered rows composite over the run's bounds at most; the
            // placements charged the spans they store themselves.
            budget.work.charge(area(bounds))?;
        }
        let item = Item {
            bounds,
            bands: self.bands(bounds),
            serial,
            position,
            work,
            kind: ItemKind::Run(Run {
                mark,
                glyphs,
                shared,
                gather,
            }),
        };
        self.push(group, item, budget)
    }
}

/// Whether two placements' marks composite alike.
fn same(a: &Mark, b: &Mark) -> bool {
    a.cover.clip == b.cover.clip
        && a.paint == b.paint
        && a.compositing == b.compositing
        && a.select == b.select
        && a.records == b.records
        && a.boundary == b.boundary
        && a.words == b.words
}

/// The rectangles where two placements' bounds meet, both ways, with each
/// placement's range set; `None` when there are more than [`MEETINGS`] per
/// placement.
pub(crate) fn meetings(glyphs: &mut [Glyph]) -> Option<Vec<DeviceRect>> {
    let limit = glyphs.len().saturating_mul(MEETINGS);
    let mut order: Vec<usize> = (0..glyphs.len()).collect();
    order.sort_unstable_by_key(|&i| glyphs.get(i).map_or(0, |g| g.bounds.x0));
    let mut active: Vec<(usize, DeviceRect)> = Vec::new();
    let mut pairs: Vec<(usize, DeviceRect)> = Vec::new();
    for &i in &order {
        let bounds = glyphs.get(i)?.bounds;
        active.retain(|(_, other)| other.x1 > bounds.x0);
        for &(j, other) in &active {
            let meet = intersect(bounds, other);
            if meet.is_empty() {
                continue;
            }
            if pairs.len() >= limit.saturating_mul(2) {
                return None;
            }
            pairs.push((i, meet));
            pairs.push((j, meet));
        }
        active.push((i, bounds));
    }
    pairs.sort_unstable_by_key(|&(i, _)| i);
    let mut shared = Vec::with_capacity(pairs.len());
    let mut pairs = pairs.into_iter().peekable();
    for (i, glyph) in glyphs.iter_mut().enumerate() {
        let start = u32::try_from(shared.len()).ok()?;
        while let Some((_, meet)) = pairs.next_if(|&(j, _)| j == i) {
            shared.push(meet);
        }
        glyph.shared = [start, u32::try_from(shared.len()).ok()?];
    }
    Some(shared)
}
