//! Clip states (8.5.4): one prepared clip per clipping path chain and
//! content transform, with the page crop as the outermost clip of page
//! content and a shading pattern's `BBox` as the innermost where the
//! pattern clips to it (8.7.4.4). Culling uses conservative bounds computed without building the
//! clip, so whether a mark meets the region never depends on which clips
//! earlier marks built.

use super::{Planner, Scope};
use crate::plan::intersect;
use crate::prepare::{Backend, Budget, Prepared, bounds, index};
use pagura_core::{Error, Result};
use pagura_interpret::ClipId;
use pagura_raster::{DeviceRect, FillRule, Path, ShapeOptions, Transform};
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

impl<B: Backend> Planner<'_, B> {
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
    fn chain(
        &self,
        scope: &Scope,
        clip: Option<ClipId>,
        extra: Option<&Extra>,
    ) -> Result<Vec<(Arc<Path>, FillRule, Transform)>> {
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
            chain.push((Arc::clone(&self.crop), FillRule::NonZero, self.request.transform));
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
        if let Some(state) = self.clip_states().get(&key) {
            return Ok(state.bounds);
        }
        let region = self.area.region;
        let bounds = self
            .chain(scope, clip, extra)?
            .iter()
            .fold(region, |acc, (path, _, transform)| {
                intersect(acc, bounds::device(bounds::path(path, *transform), region))
            });
        self.clip_states()
            .insert(key, State { bounds, built: None });
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
        if let Some(State {
            built: Some(clipping),
            ..
        }) = self.clip_states().get(&key)
        {
            return Ok(*clipping);
        }
        let clipping = self.build(scope, (clip, extra), budget)?;
        let region = self.area.region;
        let state = self.clip_states().entry(key).or_insert(State {
            bounds: region,
            built: None,
        });
        state.built = Some(clipping);
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
            precision: self.precision,
            limits: self.limits,
        };
        let chain = self.chain(scope, clip, extra)?;
        if chain.is_empty() {
            return Ok(Clipping::All);
        }
        let uniform = chain
            .iter()
            .all(|(_, _, transform)| *transform == scope.transform);
        let prepared = if uniform {
            let paths: Vec<(&Path, FillRule)> =
                chain.iter().map(|(path, rule, _)| (&**path, *rule)).collect();
            self.backend.clip(&paths, scope.transform, &options)?
        } else {
            // A fixed view moves its content but not the crop: clip in
            // device space.
            let device: Vec<(Path, FillRule)> = chain
                .iter()
                .map(|(path, rule, transform)| (path.transformed(*transform), *rule))
                .collect();
            let paths: Vec<(&Path, FillRule)> =
                device.iter().map(|(path, rule)| (path, *rule)).collect();
            self.backend.clip(&paths, Transform::IDENTITY, &options)?
        };
        budget.bytes.charge(prepared.bytes)?;
        Ok(if prepared.bounds.is_empty() {
            Clipping::Empty
        } else {
            Clipping::Shape(prepared)
        })
    }
}
