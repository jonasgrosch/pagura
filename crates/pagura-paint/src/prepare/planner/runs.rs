//! Glyph runs (ADR 0004 §4): the placements of one text-showing operator
//! painted with one solid paint become one item. Each placement is still
//! prepared, culled and charged as a mark of its own; the run keeps their
//! shapes in content order and which of them no other placement can
//! overlap, so that execution can rasterize those together and store their
//! merged rows at once, byte-identical to one item per placement.

use super::clips::Clipping;
use super::{Planner, Scope};
use crate::plan::{Glyph, Item, ItemKind, Mark, Run, area, intersect, union};
use crate::prepare::Budget;
use pagura_core::{Error, Position, Result};
use pagura_interpret::{GlyphsNode, Paint};
use pagura_raster::DeviceRect;

/// The most placements of one run item, which bounds the placements a band
/// looks at per run.
const RUN_GLYPHS: usize = 256;

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

/// What the placements of one glyph run share once the first that meets
/// the region has been prepared in full: the clip state, soft mask and
/// mark. The others take them as they are, in the same order of charges
/// and errors, preparing only their own shapes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Memo {
    pub(crate) clip_bounds: DeviceRect,
    pub(crate) clipping: Clipping,
    pub(crate) mask: Option<u32>,
    pub(crate) mark: Mark,
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

impl Planner<'_> {
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
            // A run follows one line: the placement shares rows with the
            // one before it.
            let line = open.glyphs.last().is_some_and(|last| {
                last.bounds.y0 < placed.bounds.y1 && placed.bounds.y0 < last.bounds.y1
            });
            open.group == group
                && same(&open.mark, &mark)
                && line
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
            glyphs: Vec::with_capacity(RUN_GLYPHS),
            bounds: DeviceRect::default(),
            covered: 0,
            work: 0,
        });
        open.glyphs.push(Glyph {
            #[cfg(feature = "gpu-demo")]
            serial: placed.serial,
            shape: placed.shape,
            bounds: placed.bounds,
            bands,
            alone: false,
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
        glyphs.shrink_to_fit();
        mark_alone(&mut glyphs);
        if glyphs.iter().any(|glyph| glyph.alone) {
            // Merged rows composite over the run's bounds at most; the
            // placements charged their own pixels.
            budget.work.charge(area(bounds))?;
        }
        let item = Item {
            bounds,
            bands: self.bands(bounds),
            serial,
            position,
            work,
            kind: ItemKind::Run(Run { mark, glyphs }),
        };
        self.push(group, item, budget)
    }
}

/// Whether two placements' marks composite alike.
#[inline]
fn same(a: &Mark, b: &Mark) -> bool {
    a.cover.clip == b.cover.clip
        && a.paint == b.paint
        && a.compositing == b.compositing
        && a.select == b.select
        && a.records == b.records
        && a.boundary == b.boundary
        && a.words == b.words
}

/// Marks the placements whose bounds meet no other placement's: a sweep
/// over the bounds in order of their left column, which content order
/// usually already is.
pub(crate) fn mark_alone(glyphs: &mut [Glyph]) {
    for glyph in glyphs.iter_mut() {
        glyph.alone = true;
    }
    let mut sorted_order = Vec::new();
    if !glyphs.is_sorted_by_key(|glyph| glyph.bounds.x0) {
        sorted_order = (0..glyphs.len()).collect();
        sorted_order.sort_by_key(|&i| glyphs.get(i).map_or(0, |glyph| glyph.bounds.x0));
    }
    let mut in_order = 0..glyphs.len();
    let mut out_of_order = sorted_order.into_iter();
    let order: &mut dyn Iterator<Item = usize> = if out_of_order.len() == 0 {
        &mut in_order
    } else {
        &mut out_of_order
    };
    let mut active: Vec<usize> = Vec::new();
    for i in order {
        let Some(bounds) = glyphs.get(i).map(|glyph| glyph.bounds) else {
            continue;
        };
        active.retain(|&j| {
            glyphs
                .get(j)
                .is_some_and(|other| other.bounds.x1 > bounds.x0)
        });
        for &j in &active {
            let meets = glyphs
                .get(j)
                .is_some_and(|other| !intersect(bounds, other.bounds).is_empty());
            if meets {
                for k in [i, j] {
                    if let Some(glyph) = glyphs.get_mut(k) {
                        glyph.alone = false;
                    }
                }
            }
        }
        active.push(i);
    }
}
