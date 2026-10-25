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
use pagura_core::{Error, Meter, Position, Result};
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
            glyphs: Vec::new(),
            bounds: DeviceRect::default(),
            covered: 0,
            work: 0,
        });
        budget.bytes.reserve_capacity(&mut open.glyphs, 1)?;
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
            let released = glyphs.capacity() * size_of::<Glyph>();
            drop(glyphs);
            budget.bytes.refund(released);
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
        mark_alone(&mut glyphs, &mut budget.bytes)?;
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
/// usually already is. Each run holds at most [`RUN_GLYPHS`] placements,
/// and `display_commands` bounds their total across the page.
///
/// # Errors
///
/// Returns a `Limit` error before scratch allocation exceeds `bytes`.
pub(crate) fn mark_alone(glyphs: &mut [Glyph], bytes: &mut Meter) -> Result<()> {
    let mut sorted_order = Vec::new();
    let mut active = Vec::new();
    let marked = mark(glyphs, &mut sorted_order, &mut active, bytes);
    let released = (sorted_order.capacity() + active.capacity()) * size_of::<usize>();
    drop(sorted_order);
    drop(active);
    bytes.refund(released);
    marked
}

/// Scratch is local to one sweep, including index storage for out-of-order
/// glyphs. Unstable sorting needs no further allocation and equal left
/// columns have the same overlap result in either index order.
fn mark(
    glyphs: &mut [Glyph],
    sorted_order: &mut Vec<usize>,
    active: &mut Vec<usize>,
    bytes: &mut Meter,
) -> Result<()> {
    for glyph in glyphs.iter_mut() {
        glyph.alone = true;
    }
    if !glyphs.is_sorted_by_key(|glyph| glyph.bounds.x0) {
        bytes.reserve_capacity(sorted_order, glyphs.len())?;
        sorted_order.extend(0..glyphs.len());
        sorted_order.sort_unstable_by_key(|&i| glyphs.get(i).map_or(0, |glyph| glyph.bounds.x0));
    }
    let mut in_order = 0..glyphs.len();
    let mut out_of_order = sorted_order.iter().copied();
    let order: &mut dyn Iterator<Item = usize> = if out_of_order.len() == 0 {
        &mut in_order
    } else {
        &mut out_of_order
    };
    for i in order {
        let Some(bounds) = glyphs.get(i).map(|glyph| glyph.bounds) else {
            continue;
        };
        active.retain(|&j| {
            glyphs
                .get(j)
                .is_some_and(|other| other.bounds.x1 > bounds.x0)
        });
        for &j in &*active {
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
        bytes.reserve_capacity(active, 1)?;
        active.push(i);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Placed, Planner, mark_alone};
    use crate::plan::{Boundary, Compositing, Cover, Glyph, ItemKind, Mark};
    use crate::prepare::{Budget, ShapeBuilder};
    use crate::testing::Build;
    use pagura_core::{ErrorKind, Limits, Meter, Position, Store};
    use pagura_interpret::BlendMode;
    use pagura_raster::{Area, DeviceRect};

    fn with_planner(bytes: usize, test: impl FnOnce(&mut Planner<'_>, &mut Budget)) {
        let b = Build::new(16.0, 16.0);
        let request = b.request(b.page(), 1);
        let store = Store::new(1 << 20);
        let limits = Limits {
            render_bytes: bytes,
            ..Limits::default()
        };
        let area = Area {
            region: b.page(),
            band_height: 1,
        };
        let shapes = ShapeBuilder::new(&store, &limits);
        let mut setup = Budget::new(&Limits::default());
        let mut planner = Planner::new(
            &b.scene,
            &request,
            (&store, &limits),
            area,
            shapes,
            &mut setup,
        )
        .unwrap();
        test(&mut planner, &mut Budget::new(&limits));
    }

    fn mark() -> Mark {
        Mark {
            cover: Cover::default(),
            paint: 0,
            compositing: Compositing {
                alpha: 1.0,
                blend: BlendMode::Normal,
                alpha_is_shape: false,
                mask: None,
                overprint: 0,
            },
            select: 0,
            records: false,
            boundary: Boundary::Absent,
            words: None,
        }
    }

    fn placed() -> Placed {
        Placed {
            shape: 0,
            bounds: DeviceRect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3,
            },
            work: 0,
            serial: 1,
            position: Position::Buffer(17),
        }
    }

    #[test]
    fn gathering_reserves_only_the_capacity_it_can_afford_before_growth() {
        let unit = size_of::<Glyph>();
        with_planner(unit - 1, |planner, budget| {
            let error = planner.gather(0, mark(), placed(), budget).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            let glyphs = &planner.run.as_ref().unwrap().glyphs;
            assert_eq!(
                (glyphs.len(), glyphs.capacity(), budget.bytes.used()),
                (0, 0, 0)
            );
        });
        // Two retained glyphs fit, but the old allocation must remain
        // charged while reserving its two-glyph replacement.
        with_planner(3 * unit - 1, |planner, budget| {
            planner.gather(0, mark(), placed(), budget).unwrap();
            let error = planner.gather(0, mark(), placed(), budget).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            let glyphs = &planner.run.as_ref().unwrap().glyphs;
            assert_eq!(
                (glyphs.len(), glyphs.capacity(), budget.bytes.used()),
                (1, 1, unit)
            );
        });
    }

    /// The item and group-index capacity, and one order slot.
    fn item_bytes(planner: &Planner<'_>) -> usize {
        planner.parts.items.capacity() * size_of::<crate::plan::Item>()
            + planner.parts.groups[0].items.capacity() * size_of::<u32>()
            + size_of::<u32>()
    }

    #[test]
    fn flushed_runs_keep_capacity_charged_and_singletons_return_it() {
        with_planner(1 << 20, |planner, budget| {
            for _ in 0..3 {
                planner.gather(0, mark(), placed(), budget).unwrap();
            }
            let capacity = planner.run.as_ref().unwrap().glyphs.capacity();
            assert!(capacity > 3 && capacity < super::RUN_GLYPHS);
            let retained = capacity * size_of::<Glyph>();
            assert_eq!(budget.bytes.used(), retained);
            planner.flush(budget).unwrap();
            let ItemKind::Run(run) = &planner.parts.items[0].kind else {
                panic!("expected run")
            };
            assert_eq!(run.glyphs.capacity(), capacity);
            assert_eq!(budget.bytes.used(), retained + item_bytes(planner));
        });
        with_planner(1 << 20, |planner, budget| {
            planner.gather(0, mark(), placed(), budget).unwrap();
            planner.flush(budget).unwrap();
            assert!(matches!(planner.parts.items[0].kind, ItemKind::Mark(_)));
            assert_eq!(budget.bytes.used(), item_bytes(planner));
        });
    }

    #[test]
    fn overlap_scratch_reserves_before_allocation_and_is_returned_on_refusal() {
        let mut glyphs = [2, 1].map(|x| Glyph {
            #[cfg(feature = "gpu-demo")]
            serial: x,
            shape: x,
            bounds: DeviceRect {
                x0: x,
                y0: 1,
                x1: x + 2,
                y1: 3,
            },
            bands: [0, 0],
            alone: false,
        });
        let mut sorted_order = Vec::new();
        let mut active = Vec::new();
        let mut bytes = Meter::new(2 * size_of::<usize>() - 1, "test bytes");
        let error =
            super::mark(&mut glyphs, &mut sorted_order, &mut active, &mut bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "test bytes")
        );
        assert_eq!(
            (sorted_order.capacity(), active.capacity(), bytes.used()),
            (0, 0, 0)
        );
        assert_eq!(glyphs.map(|g| g.shape), [2, 1]);

        // Sorting fits, then the active set refuses its first word. The
        // already allocated sorting scratch must be dropped and refunded.
        let mut bytes = Meter::new(2 * size_of::<usize>(), "test bytes");
        let error = mark_alone(&mut glyphs, &mut bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "test bytes")
        );
        assert_eq!(bytes.used(), 0);
    }

    #[test]
    fn overlap_sweep_preserves_content_order_and_equal_left_edges() {
        let mut glyphs: Vec<_> = (0..super::RUN_GLYPHS)
            .map(|i| {
                let i = u32::try_from(i).unwrap();
                let (x, y) = ((i * 17) % 100, (i * 23) % 71);
                Glyph {
                    #[cfg(feature = "gpu-demo")]
                    serial: i,
                    shape: i,
                    bounds: DeviceRect {
                        x0: x,
                        y0: y,
                        x1: x + 3,
                        y1: y + 4,
                    },
                    bands: [0, 0],
                    alone: false,
                }
            })
            .collect();
        let expected: Vec<_> = glyphs
            .iter()
            .enumerate()
            .map(|(i, glyph)| {
                !glyphs.iter().enumerate().any(|(j, other)| {
                    i != j && !crate::plan::intersect(glyph.bounds, other.bounds).is_empty()
                })
            })
            .collect();
        let mut bytes = Meter::new(1 << 20, "test bytes");
        mark_alone(&mut glyphs, &mut bytes).unwrap();
        assert_eq!(bytes.used(), 0);
        for (i, (glyph, alone)) in glyphs.iter().zip(expected).enumerate() {
            assert_eq!(glyph.shape, u32::try_from(i).unwrap());
            assert_eq!(glyph.alone, alone);
        }
    }
}
