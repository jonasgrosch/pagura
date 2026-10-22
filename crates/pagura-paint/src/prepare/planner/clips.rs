//! Clip states (8.5.4): one prepared clip per clipping path chain and
//! content transform, with the page crop as the outermost clip of page
//! content and a shading pattern's `BBox` as the innermost where the
//! pattern clips to it (8.7.4.4). Culling uses conservative bounds computed without building the
//! clip, so whether a mark meets the region never depends on which clips
//! earlier marks built.

use super::{Planner, Scope};
use crate::hash::{FastHasher, FastMap};
use crate::plan::intersect;
use crate::prepare::{Budget, Prepared, bounds, index};
use pagura_core::{Error, Result};
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
    /// The path.
    pub(crate) path: Arc<Path>,
    /// Its transform to device space.
    pub(crate) transform: Transform,
}

/// The paths of a clip state, outermost first, with their rules and
/// transforms.
type Chain = Vec<(Arc<Path>, FillRule, Transform)>;

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

    /// The paths of `clip`'s chain, outermost first, with the transform of
    /// each (the crop is in default user space).
    fn chain(&self, scope: &Scope, clip: Option<ClipId>, extra: Option<&Extra>) -> Result<Chain> {
        let mut chain = Vec::new();
        if let Some(extra) = extra {
            chain.push((Arc::clone(&extra.path), FillRule::NonZero, extra.transform));
        }
        let mut current = clip;
        while let Some(id) = current {
            if chain.len() >= self.limits.mask_sources {
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
            chain.push((Arc::clone(&node.path), node.rule, scope.transform));
            current = node.parent;
        }
        if scope.crop {
            chain.push((
                Arc::clone(&self.crop),
                FillRule::NonZero,
                self.request.transform,
            ));
        }
        chain.reverse();
        Ok(chain)
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
        let bounds =
            self.chain(scope, clip, extra)?
                .iter()
                .fold(region, |acc, (path, _, transform)| {
                    let hull = bounds::path(path, *transform).map(|h| bounds::expand(h, 1.0));
                    intersect(acc, bounds::device(hull, region))
                });
        let state = State {
            bounds,
            built: None,
        };
        self.clip_states().insert(key, state);
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
        let clipping = self.build(scope, (clip, extra), budget)?;
        let region = self.area.region;
        let state = self.clip_states().entry(key).or_insert(State {
            bounds: region,
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
        let options = ShapeOptions {
            area: self.area,
            limits: self.limits,
        };
        let chain = self.chain(scope, clip, extra)?;
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
            return Ok(clipping);
        }
        let uniform = chain
            .iter()
            .all(|(_, _, transform)| *transform == scope.transform);
        let prepared = if uniform {
            let paths: Vec<(&Path, FillRule)> = chain
                .iter()
                .map(|(path, rule, _)| (&**path, *rule))
                .collect();
            self.shapes.clip(&paths, scope.transform, &options)?
        } else {
            // A fixed view moves its content but not the crop: clip in
            // device space.
            let device: Vec<(Path, FillRule)> = chain
                .iter()
                .map(|(path, rule, transform)| (path.transformed(*transform), *rule))
                .collect();
            let paths: Vec<(&Path, FillRule)> =
                device.iter().map(|(path, rule)| (path, *rule)).collect();
            self.shapes.clip(&paths, Transform::IDENTITY, &options)?
        };
        budget.bytes.charge(prepared.bytes)?;
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
}

/// The bits of `t`.
fn bits(t: Transform) -> [u64; 6] {
    [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits)
}

/// A hash of the bits of `chain`'s rules, transforms and segments.
fn chain_hash(chain: &Chain) -> u64 {
    let mut hasher = FastHasher::default();
    for (path, rule, transform) in chain {
        rule.hash(&mut hasher);
        bits(*transform).hash(&mut hasher);
        for segment in path.segments() {
            segment_bits(*segment).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Whether two chains are the same paths, rules and transforms, bit for bit.
fn same_chain(a: &Chain, b: &Chain) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((a, a_rule, a_t), (b, b_rule, b_t))| {
            a_rule == b_rule
                && bits(*a_t) == bits(*b_t)
                && (Arc::ptr_eq(a, b)
                    || a == b
                        && a.segments()
                            .iter()
                            .zip(b.segments())
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
    }
}
