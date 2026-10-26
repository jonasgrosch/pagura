//! Clip states (8.5.4): one prepared clip per clipping path chain and
//! content transform, with the page crop as the outermost clip of page
//! content and a shading pattern's `BBox` as the innermost where the
//! pattern clips to it (8.7.4.4). Culling uses conservative bounds computed without building the
//! clip, so whether a mark meets the region never depends on which clips
//! earlier marks built.

use super::{Planner, Scope};
use crate::plan::intersect;
use crate::prepare::{Budget, Prepared, bounds, index};
use pagura_core::hash::{FastHasher, FastMap};
use pagura_core::{Error, Meter, Result};
use pagura_interpret::ClipId;
use pagura_raster::{DeviceRect, FillRule, Path, Segment, ShapeOptions, Transform};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The identity of a clip state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    clip: Option<ClipId>,
    crop: bool,
    transform: [u64; 6],
    extra: Option<(u32, [u64; 6])>,
}

/// A path that clips a mark besides its clipping path chain: a shading
/// pattern's `BBox` under its own transform.
#[derive(Debug, Clone)]
pub(crate) struct Extra {
    /// The paint it belongs to, for the key.
    pub(crate) paint: u32,
    /// The canonical rectangle, built only after the mark meets the region.
    pub(crate) bbox: pagura_core::Rect,
    /// Its transform to device space.
    pub(crate) transform: Transform,
}

/// One path of a clip state.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    path: Arc<Path>,
    rule: FillRule,
    /// Its space to device pixels.
    transform: Transform,
    /// Owned path allocation of an extra rectangle; scene/crop paths have
    /// their owning phase and contribute no second payload charge here.
    owned: usize,
}

/// The paths of a clip state, outermost first.
type Chain = Vec<Link>;

/// Prepared clips by the hash of their chains. Content often sets the same
/// clipping path again as a new clip; clip states with equal chains share
/// one prepared clip.
pub(crate) type Built = FastMap<u64, Vec<(Chain, Clipping)>>;

/// A clip state's bounds and, once built, its prepared clip.
#[derive(Debug, Clone, Copy)]
pub(crate) struct State {
    bounds: DeviceRect,
    built: Option<Clipping>,
}

/// What a clip state keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Clipping {
    /// Everything: no clipping path.
    All,
    /// Nothing within the area.
    Empty,
    /// The pixels of a prepared clip.
    Shape(Prepared),
}

impl Planner<'_> {
    fn key(scope: &Scope, clip: Option<ClipId>, extra: Option<&Extra>) -> Key {
        let bits = |t: Transform| [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits);
        Key {
            clip,
            crop: scope.crop,
            transform: scope.transform_bits(),
            extra: extra.map(|e| (e.paint, bits(e.transform))),
        }
    }

    /// Visits `clip` from its innermost path to the crop, with each
    /// transform. Bounds can intersect these without allocating a chain.
    fn walk_chain(
        &self,
        scope: &Scope,
        clip: Option<ClipId>,
        extra: bool,
        mut visit: impl FnMut(Link) -> Result<()>,
    ) -> Result<()> {
        let mut count = usize::from(extra);
        let mut current = clip;
        while let Some(id) = current {
            if count >= self.limits.mask_sources {
                return Err(Error::limit("clip path budget exceeded"));
            }
            let node = self
                .scene
                .clips
                .get(index(id.0))
                .ok_or(Error::invalid_argument("scene clip out of range"))?;
            if node.parent.is_some_and(|parent| parent.0 >= id.0) {
                return Err(Error::invalid_argument("scene clip chain is not ordered"));
            }
            visit(Link {
                path: Arc::clone(&node.path),
                rule: node.rule,
                transform: scope.transform,
                owned: 0,
            })?;
            count += 1;
            current = node.parent;
        }
        if scope.crop {
            visit(Link {
                path: Arc::clone(&self.crop),
                rule: FillRule::NonZero,
                transform: self.request.transform,
                owned: 0,
            })?;
        }
        Ok(())
    }

    /// Conservative bounds of the pixels `clip` keeps in `scope`.
    pub(crate) fn clip_bounds(
        &mut self,
        scope: &Scope,
        clip: Option<ClipId>,
        extra: Option<&Extra>,
    ) -> Result<DeviceRect> {
        let key = Self::key(scope, clip, extra);
        if let Some((last, state)) = self.last_clip
            && last == key
        {
            return Ok(state.bounds);
        }
        if let Some(&state) = self.clip_states().get(&key) {
            self.last_clip = Some((key, state));
            return Ok(state.bounds);
        }
        let region = self.area.region;
        // A pixel wider: a clipping path without area still keeps the
        // pixels its boundary touches.
        let mut bounds = region;
        if let Some(extra) = extra {
            let hull = bounds::rect(extra.bbox, extra.transform).map(|h| bounds::expand(h, 1.0));
            bounds = intersect(bounds, bounds::device(hull, region));
        }
        self.walk_chain(scope, clip, extra.is_some(), |link| {
            let hull = bounds::path(&link.path, link.transform).map(|h| bounds::expand(h, 1.0));
            bounds = intersect(bounds, bounds::device(hull, region));
            Ok(())
        })?;
        let state = State {
            bounds,
            built: None,
        };
        self.last_clip = Some((key, state));
        Ok(bounds)
    }

    /// The prepared clip of `clip` in `scope`, built once.
    pub(crate) fn clip(
        &mut self,
        scope: &Scope,
        (clip, extra): (Option<ClipId>, Option<&Extra>),
        budget: &mut Budget,
    ) -> Result<Clipping> {
        let key = Self::key(scope, clip, extra);
        if let Some((
            last,
            State {
                built: Some(clipping),
                ..
            },
        )) = self.last_clip
            && last == key
        {
            return Ok(clipping);
        }
        if let Some(
            &state @ State {
                built: Some(clipping),
                ..
            },
        ) = self.clip_states().get(&key)
        {
            self.last_clip = Some((key, state));
            return Ok(clipping);
        }
        let bounds = self.clip_bounds(scope, clip, extra)?;
        if !self.clips.contains_key(&key) {
            budget.bytes.reserve_map_capacity(&mut self.clips, 1)?;
        }
        let clipping = self.build(scope, (clip, extra), budget)?;
        let state = self.clip_states().entry(key).or_insert(State {
            bounds,
            built: None,
        });
        state.built = Some(clipping);
        let state = *state;
        self.last_clip = Some((key, state));
        Ok(clipping)
    }

    fn build(
        &mut self,
        scope: &Scope,
        (clip, extra): (Option<ClipId>, Option<&Extra>),
        budget: &mut Budget,
    ) -> Result<Clipping> {
        let mut chain = self.chain(scope, (clip, extra), &mut budget.bytes)?;
        chain.reverse();
        if chain.is_empty() {
            return Ok(Clipping::All);
        }
        let hash = chain_hash(&chain);
        let built = self.built_clips.get(&hash).and_then(|built| {
            built
                .iter()
                .find(|(other, _)| same_chain(other, &chain))
                .map(|&(_, clipping)| clipping)
        });
        if let Some(clipping) = built {
            let released = chain_bytes(&chain);
            drop(chain);
            budget.bytes.refund(released);
            return Ok(clipping);
        }
        let prepared = (|| {
            if !self.built_clips.contains_key(&hash) {
                budget
                    .bytes
                    .reserve_map_capacity(&mut self.built_clips, 1)?;
            }
            budget
                .bytes
                .reserve_capacity(self.built_clips.entry(hash).or_default(), 1)?;
            self.clip_geometry(scope, &chain, &mut budget.bytes)
        })();
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let released = chain_bytes(&chain);
                drop(chain);
                budget.bytes.refund(released);
                return Err(error);
            }
        };
        let clipping = if prepared.bounds.is_empty() {
            Clipping::Empty
        } else {
            Clipping::Shape(prepared)
        };
        self.built_clips
            .entry(hash)
            .or_default()
            .push((chain, clipping));
        Ok(clipping)
    }

    fn chain(
        &self,
        scope: &Scope,
        (clip, extra): (Option<ClipId>, Option<&Extra>),
        bytes: &mut Meter,
    ) -> Result<Chain> {
        let before = bytes.used();
        let mut chain = Vec::new();
        let built = (|| {
            if let Some(extra) = extra {
                bytes.reserve_capacity(&mut chain, 1)?;
                bytes.charge(2 * size_of::<usize>() + size_of::<Path>())?;
                let path = super::marks::bbox_path(extra.bbox, self.limits, bytes)?;
                let owned = 2 * size_of::<usize>() + size_of::<Path>() + path.retained_bytes();
                chain.push(Link {
                    path: Arc::new(path),
                    rule: FillRule::NonZero,
                    transform: extra.transform,
                    owned,
                });
            }
            self.walk_chain(scope, clip, extra.is_some(), |link| {
                bytes.reserve_capacity(&mut chain, 1)?;
                chain.push(link);
                Ok(())
            })
        })();
        if let Err(error) = built {
            drop(chain);
            bytes.refund(bytes.used() - before);
            return Err(error);
        }
        Ok(chain)
    }

    /// Source copies and reference lists are temporary; the chain, table
    /// entry and prepared clip stay charged to the same cumulative meter.
    fn clip_geometry(
        &mut self,
        scope: &Scope,
        chain: &[Link],
        bytes: &mut Meter,
    ) -> Result<Prepared> {
        let uniform = chain.iter().all(|link| link.transform == scope.transform);
        if uniform {
            let mut paths = Vec::new();
            bytes.reserve_capacity(&mut paths, chain.len())?;
            paths.extend(chain.iter().map(|link| (&*link.path, link.rule)));
            let limits = pagura_core::Limits {
                render_bytes: bytes.remaining(),
                ..*self.limits
            };
            let options = ShapeOptions {
                area: self.area,
                limits: &limits,
            };
            let prepared = self.shapes.clip(&paths, scope.transform, &options, bytes);
            let released = paths.capacity() * size_of::<(&Path, FillRule)>();
            drop(paths);
            bytes.refund(released);
            prepared
        } else {
            // A fixed view moves its content but not the crop: clip in
            // device space. Source copies stay charged while the prepared
            // clip is constructed beside them.
            let mut device = Vec::new();
            let prepared = (|| {
                bytes.reserve_capacity(&mut device, chain.len())?;
                for link in chain {
                    device.push((
                        link.path.transformed_with_bytes(link.transform, bytes)?,
                        link.rule,
                    ));
                }
                let mut paths = Vec::new();
                bytes.reserve_capacity(&mut paths, device.len())?;
                paths.extend(device.iter().map(|(path, rule)| (path, *rule)));
                let limits = pagura_core::Limits {
                    render_bytes: bytes.remaining(),
                    ..*self.limits
                };
                let options = ShapeOptions {
                    area: self.area,
                    limits: &limits,
                };
                let prepared = self
                    .shapes
                    .clip(&paths, Transform::IDENTITY, &options, bytes);
                let released = paths.capacity() * size_of::<(&Path, FillRule)>();
                drop(paths);
                bytes.refund(released);
                prepared
            })();
            let released = device.capacity() * size_of::<(Path, FillRule)>()
                + device
                    .iter()
                    .map(|(path, _)| path.retained_bytes())
                    .sum::<usize>();
            drop(device);
            bytes.refund(released);
            prepared
        }
    }
}

/// Retained chains and collision lists, besides the table's own storage.
pub(super) fn retained_bytes(built: &Built, bytes: &Meter) -> Result<usize> {
    Ok(bytes.map_bytes(built)?
        + built
            .values()
            .map(|list| {
                list.capacity() * size_of::<(Chain, Clipping)>()
                    + list
                        .iter()
                        .map(|(chain, _)| chain_bytes(chain))
                        .sum::<usize>()
            })
            .sum::<usize>())
}

fn chain_bytes(chain: &Chain) -> usize {
    chain.capacity() * size_of::<Link>() + chain.iter().map(|link| link.owned).sum::<usize>()
}

/// The bits of `t`.
fn bits(t: Transform) -> [u64; 6] {
    [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits)
}

/// A hash of the bits of `chain`'s rules, transforms and segments.
fn chain_hash(chain: &Chain) -> u64 {
    let mut hasher = FastHasher::default();
    for link in chain {
        link.rule.hash(&mut hasher);
        bits(link.transform).hash(&mut hasher);
        for segment in link.path.segments() {
            segment_bits(*segment).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Whether two chains are the same paths, rules and transforms, bit for bit.
fn same_chain(a: &Chain, b: &Chain) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.rule == b.rule
                && bits(a.transform) == bits(b.transform)
                && (Arc::ptr_eq(&a.path, &b.path)
                    || a.path == b.path
                        && a.path
                            .segments()
                            .iter()
                            .zip(b.path.segments())
                            .all(|(&a, &b)| segment_bits(a) == segment_bits(b)))
        })
}

/// A segment's kind and the bits of its points.
fn segment_bits(segment: Segment) -> (u8, [u64; 6]) {
    let bits = |p: pagura_raster::Point| (p.x.to_bits(), p.y.to_bits());
    match segment {
        Segment::MoveTo(p) => {
            let (x, y) = bits(p);
            (0, [x, y, 0, 0, 0, 0])
        }
        Segment::LineTo(p) => {
            let (x, y) = bits(p);
            (1, [x, y, 0, 0, 0, 0])
        }
        Segment::CubicTo { c1, c2, end } => {
            let ((x1, y1), (x2, y2), (x, y)) = (bits(c1), bits(c2), bits(end));
            (2, [x1, y1, x2, y2, x, y])
        }
        Segment::Close => (3, [0; 6]),
        Segment::QuadTo { control, end } => {
            let ((x1, y1), (x, y)) = (bits(control), bits(end));
            (4, [x1, y1, x, y, 0, 0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::ShapeBuilder;
    use crate::testing::Build;
    use pagura_core::{Limits, Position, Rect, Store};
    use pagura_raster::Area;

    fn with_planner(test: impl FnOnce(Planner<'_>, Scope, Budget)) {
        let b = Build::new(4.0, 4.0);
        let request = b.request(b.page(), 1);
        let store = Store::new(1 << 20);
        let limits = Limits::default();
        let mut budget = Budget::new(&limits);
        let planner = Planner::new(
            &b.scene,
            &request,
            (&store, &limits),
            Area {
                region: b.page(),
                band_height: 1,
            },
            ShapeBuilder::new(&store, &limits),
            &mut budget,
        )
        .unwrap();
        let scope = Scope {
            group: 0,
            transform: Transform::IDENTITY,
            crop: false,
            bounds: b.page(),
            layout: planner.parts.groups[0].layout,
            plain: true,
            records: false,
            separated: false,
            masked: false,
            paint: None,
            depth: 0,
            upright: 0.0,
        };
        test(planner, scope, budget);
    }

    fn extra(paint: u32) -> Extra {
        Extra {
            paint,
            bbox: Rect::from_corners(0.25, 0.25, 3.75, 3.75).unwrap(),
            transform: Transform::IDENTITY,
        }
    }

    #[test]
    fn culling_uses_no_owned_path_or_table_capacity() {
        with_planner(|mut planner, scope, budget| {
            let before = budget.bytes.used();
            planner.clip_bounds(&scope, None, Some(&extra(0))).unwrap();
            assert_eq!(budget.bytes.used(), before);
            assert_eq!(planner.clips.capacity(), 0);
            assert!(planner.built_clips.is_empty());
        });
    }

    #[test]
    fn building_keeps_the_allocation_free_culling_bounds() {
        with_planner(|mut planner, scope, mut budget| {
            let extra = Extra {
                bbox: Rect::from_corners(1.25, 1.25, 1.75, 1.75).unwrap(),
                ..extra(0)
            };
            let before = planner.clip_bounds(&scope, None, Some(&extra)).unwrap();
            assert_ne!(before, planner.area.region);
            planner
                .clip(&scope, (None, Some(&extra)), &mut budget)
                .unwrap();
            assert_eq!(
                planner.clip_bounds(&scope, None, Some(&extra)).unwrap(),
                before
            );
        });
    }

    #[test]
    fn duplicate_extra_chains_return_their_temporary_path_and_slots() {
        with_planner(|mut planner, scope, mut budget| {
            let first = planner
                .clip(&scope, (None, Some(&extra(0))), &mut budget)
                .unwrap();
            let before = budget.bytes.used();
            let table = budget.bytes.map_bytes(&planner.clips).unwrap();
            let second = planner
                .clip(&scope, (None, Some(&extra(1))), &mut budget)
                .unwrap();
            assert_eq!(first, second);
            assert_eq!(planner.built_clips.len(), 1);
            assert_eq!(planner.built_clips.values().next().unwrap().len(), 1);
            assert_eq!(
                budget.bytes.used() - before,
                budget.bytes.map_bytes(&planner.clips).unwrap() - table
            );
            let walked = planner.finish(&mut budget.bytes).unwrap();
            let shapes = walked.shapes.finish(&mut budget.bytes).unwrap();
            let names = 2 * size_of::<usize>(); // empty and spots share one Arc
            assert_eq!(
                budget.bytes.used(),
                walked.parts.groups.capacity() * size_of::<crate::plan::GroupPlan>()
                    + names
                    + shapes.retained_bytes()
            );
        });
    }

    #[test]
    fn chain_growth_is_refused_before_allocating_owned_geometry() {
        with_planner(|mut planner, scope, mut budget| {
            // Reserve just the state table, then leave less than one Link.
            budget
                .bytes
                .reserve_map_capacity(&mut planner.clips, 1)
                .unwrap();
            let used = budget.bytes.used();
            let mut tight = Meter::new(
                used + size_of::<Link>() - 1,
                "render memory budget exceeded",
            );
            tight.charge(used).unwrap();
            budget.bytes = tight;
            let error = planner
                .clip(&scope, (None, Some(&extra(0))), &mut budget)
                .unwrap_err();
            assert_eq!(error.message, "render memory budget exceeded");
            assert_eq!(error.position, Position::Unknown);
            assert!(planner.built_clips.is_empty());
            assert!(planner.clips.is_empty());
            assert_eq!(budget.bytes.used(), used);
        });
    }
}
