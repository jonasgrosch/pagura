//! Interning of the shared values marks carry. Composite states and solid
//! colours of up to four components are interned by value, so equal values
//! the interpreter built separately share one table entry; other paints,
//! pens and clips by identity. The tables hold every value interned by
//! identity, so its address names it for the whole recording.

use super::owned;
use crate::model::{BlendMode, Clip, Composite, MaskId, Overprint, Paint, Pen};
use crate::scene::{ClipId, ClipNode, PaintId, PenId, StateId};
use pagura_color::Intent;
use pagura_core::hash::FastMap;
use pagura_core::{Error, Meter, Result};
use std::sync::Arc;

/// The interned tables of a recording.
#[derive(Debug, Default)]
pub(super) struct Interner {
    clips: Vec<ClipNode>,
    clip_ids: FastMap<usize, ClipId>,
    /// The interned clips, kept so their addresses stay theirs.
    clip_arcs: Vec<Arc<Clip>>,
    states: Vec<Arc<Composite>>,
    state_ids: FastMap<StateKey, StateId>,
    paints: Vec<Arc<Paint>>,
    solid_ids: FastMap<SolidKey, PaintId>,
    /// Paints interned by identity, which `paints` holds.
    paint_ids: FastMap<usize, PaintId>,
    pens: Vec<Arc<Pen>>,
    pen_ids: FastMap<usize, PenId>,
    /// The ICC profiles of the recorded colour spaces, charged once each.
    pub(super) profiles: owned::Profiles,
    /// The images drawn, charged once each.
    pub(super) images: owned::Images,
}

/// A composite state by value; functions by identity, which the interned
/// state holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StateKey {
    alpha: u64,
    blend: BlendMode,
    soft_mask: Option<MaskId>,
    alpha_is_shape: bool,
    overprint: Overprint,
    transfer: usize,
    intent: u8,
    black_generation: usize,
    undercolor_removal: usize,
}

impl StateKey {
    fn of(state: &Composite) -> Self {
        let function = |f: Option<&Arc<pagura_color::Function>>| f.map_or(0, address);
        Self {
            alpha: state.alpha.to_bits(),
            blend: state.blend,
            soft_mask: state.soft_mask,
            alpha_is_shape: state.alpha_is_shape,
            overprint: state.overprint,
            transfer: state.transfer.as_ref().map_or(0, address),
            intent: match state.conversion.intent {
                Intent::Perceptual => 0,
                Intent::Relative => 1,
                Intent::Saturation => 2,
                Intent::Absolute => 3,
            },
            black_generation: function(state.conversion.black_generation.as_ref()),
            undercolor_removal: function(state.conversion.undercolor_removal.as_ref()),
        }
    }
}

/// A solid colour of up to four components by value: its space by
/// identity, which the interned paint holds, and its components' bits
/// (signed zeros distinguished).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SolidKey {
    space: usize,
    len: usize,
    bits: [u64; 4],
}

impl SolidKey {
    fn of(paint: &Paint) -> Option<Self> {
        let Paint::Solid(color) = paint else {
            return None;
        };
        let values = color.components.as_slice();
        let mut bits = [0; 4];
        if values.len() > bits.len() {
            return None;
        }
        for (bits, value) in bits.iter_mut().zip(values) {
            *bits = value.to_bits();
        }
        Some(Self {
            space: address(&color.space),
            len: values.len(),
            bits,
        })
    }
}

/// The tables of a finished recording.
pub(super) type Tables = (
    Vec<ClipNode>,
    Vec<Arc<Composite>>,
    Vec<Arc<Paint>>,
    Vec<Arc<Pen>>,
);

fn index(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| Error::limit("scene table size exceeded"))
}

fn address<T>(value: &Arc<T>) -> usize {
    Arc::as_ptr(value).addr()
}

impl Interner {
    pub(super) fn into_tables(self, bytes: &mut Meter) -> Result<Tables> {
        let released = bytes.map_bytes(&self.clip_ids)?
            + bytes.map_bytes(&self.state_ids)?
            + bytes.map_bytes(&self.solid_ids)?
            + bytes.map_bytes(&self.paint_ids)?
            + bytes.map_bytes(&self.pen_ids)?
            + self.clip_arcs.capacity() * size_of::<Arc<Clip>>()
            + self.clip_arcs.len() * (owned::ARC + size_of::<Clip>())
            + self.profiles.index_bytes(bytes)?
            + self.images.index_bytes(bytes)?;
        let Self {
            clips,
            states,
            paints,
            pens,
            clip_ids,
            clip_arcs,
            state_ids,
            solid_ids,
            paint_ids,
            pen_ids,
            profiles,
            images,
        } = self;
        drop((
            clip_ids, clip_arcs, state_ids, solid_ids, paint_ids, pen_ids, profiles, images,
        ));
        bytes.refund(released);
        Ok((clips, states, paints, pens))
    }

    pub(super) fn state(&mut self, state: &Arc<Composite>, bytes: &mut Meter) -> Result<StateId> {
        self.state_with(state, || Arc::clone(state), bytes)
    }

    pub(super) fn page_state(&mut self, bytes: &mut Meter) -> Result<StateId> {
        let state = Composite::default();
        self.state_with(&state, || Arc::new(state.clone()), bytes)
    }

    /// Reserves both the payload and tables before creating an owned Arc.
    fn state_with(
        &mut self,
        state: &Composite,
        retain: impl FnOnce() -> Arc<Composite>,
        bytes: &mut Meter,
    ) -> Result<StateId> {
        let key = StateKey::of(state);
        if let Some(&id) = self.state_ids.get(&key) {
            return Ok(id);
        }
        bytes.charge(owned::composite(state))?;
        let id = StateId(index(self.states.len())?);
        bytes.reserve_capacity(&mut self.states, 1)?;
        bytes.reserve_map_capacity(&mut self.state_ids, 1)?;
        self.states.push(retain());
        self.state_ids.insert(key, id);
        Ok(id)
    }

    pub(super) fn paint(&mut self, paint: &Arc<Paint>, bytes: &mut Meter) -> Result<PaintId> {
        let solid = SolidKey::of(paint);
        let known = match &solid {
            Some(key) => self.solid_ids.get(key),
            None => self.paint_ids.get(&address(paint)),
        };
        if let Some(&id) = known {
            return Ok(id);
        }
        bytes.charge(owned::paint(paint))?;
        self.profiles.charge(owned::paint_space(paint), bytes)?;
        let id = PaintId(index(self.paints.len())?);
        bytes.reserve_capacity(&mut self.paints, 1)?;
        match &solid {
            Some(_) => bytes.reserve_map_capacity(&mut self.solid_ids, 1)?,
            None => bytes.reserve_map_capacity(&mut self.paint_ids, 1)?,
        }
        self.paints.push(Arc::clone(paint));
        match solid {
            Some(key) => self.solid_ids.insert(key, id),
            None => self.paint_ids.insert(address(paint), id),
        };
        Ok(id)
    }

    pub(super) fn pen(&mut self, pen: &Arc<Pen>, bytes: &mut Meter) -> Result<PenId> {
        if let Some(&id) = self.pen_ids.get(&address(pen)) {
            return Ok(id);
        }
        bytes.charge(owned::pen(pen))?;
        let id = PenId(index(self.pens.len())?);
        bytes.reserve_capacity(&mut self.pens, 1)?;
        bytes.reserve_map_capacity(&mut self.pen_ids, 1)?;
        self.pens.push(Arc::clone(pen));
        self.pen_ids.insert(address(pen), id);
        Ok(id)
    }

    /// Interns `clip` and the chain above it.
    pub(super) fn clip(&mut self, clip: &Arc<Clip>, bytes: &mut Meter) -> Result<ClipId> {
        if let Some(&id) = self.clip_ids.get(&address(clip)) {
            return Ok(id);
        }
        // Walk up to the first interned ancestor, then intern downwards.
        let mut chain = Vec::new();
        let result = self.clip_chain(clip, &mut chain, bytes);
        let released = chain.capacity() * size_of::<&Arc<Clip>>();
        drop(chain);
        bytes.refund(released);
        result
    }

    fn clip_chain<'a>(
        &mut self,
        clip: &'a Arc<Clip>,
        chain: &mut Vec<&'a Arc<Clip>>,
        bytes: &mut Meter,
    ) -> Result<ClipId> {
        bytes.reserve_capacity(chain, 1)?;
        chain.push(clip);
        let mut parent = None;
        let mut next = clip.parent();
        while let Some(node) = next {
            if let Some(&id) = self.clip_ids.get(&address(node)) {
                parent = Some(id);
                break;
            }
            bytes.reserve_capacity(chain, 1)?;
            chain.push(node);
            next = node.parent();
        }
        for &node in chain.iter().rev() {
            bytes.charge(owned::path(node.path()))?;
            // These keep recording identities alive, but only paths survive
            // into the scene. Their Arc roots are refunded with clip_arcs.
            bytes.charge(owned::ARC + size_of::<Clip>())?;
            let id = ClipId(index(self.clips.len())?);
            bytes.reserve_capacity(&mut self.clips, 1)?;
            bytes.reserve_capacity(&mut self.clip_arcs, 1)?;
            bytes.reserve_map_capacity(&mut self.clip_ids, 1)?;
            self.clips.push(ClipNode {
                parent,
                path: Arc::clone(node.path()),
                rule: node.rule(),
            });
            self.clip_ids.insert(address(node), id);
            self.clip_arcs.push(Arc::clone(node));
            parent = Some(id);
        }
        parent.ok_or(Error::invalid_argument("empty clip chain"))
    }
}
