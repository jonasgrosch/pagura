//! Interning of the shared values marks carry: by `Arc` identity first,
//! then composite states and solid colours by value, so equal values the
//! interpreter built separately share one table entry. The interner holds
//! every `Arc` it has seen, so an identity is never reused while interned.

use crate::hash::FastMap;
use crate::model::{BlendMode, Clip, Composite, MaskId, Overprint, Paint, Pen};
use crate::scene::{ClipId, ClipNode, PaintId, PenId, StateId};
use pagura_color::Intent;
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
    state_ids: FastMap<usize, StateId>,
    state_values: FastMap<StateKey, StateId>,
    paints: Vec<Arc<Paint>>,
    paint_ids: FastMap<usize, PaintId>,
    paint_values: FastMap<(usize, Vec<u64>), PaintId>,
    /// States and paints interned as aliases of equal values, kept alive so
    /// their addresses stay theirs.
    state_aliases: Vec<Arc<Composite>>,
    paint_aliases: Vec<Arc<Paint>>,
    pens: Vec<Arc<Pen>>,
    pen_ids: FastMap<usize, PenId>,
    /// The last states, paints and clips interned, by address: marks
    /// mostly repeat their neighbours', so these are checked before the
    /// maps.
    recent_state: Option<(usize, StateId)>,
    recent_paints: [Option<(usize, PaintId)>; 2],
    recent_clip: Option<(usize, ClipId)>,
}

/// A composite state by value; functions by identity.
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
    pub(super) fn into_tables(self) -> Tables {
        (self.clips, self.states, self.paints, self.pens)
    }

    pub(super) fn state(&mut self, state: &Arc<Composite>, bytes: &mut Meter) -> Result<StateId> {
        let address = address(state);
        if let Some((recent, id)) = self.recent_state
            && recent == address
        {
            return Ok(id);
        }
        let id = self.state_by_value(state, bytes)?;
        self.recent_state = Some((address, id));
        Ok(id)
    }

    fn state_by_value(&mut self, state: &Arc<Composite>, bytes: &mut Meter) -> Result<StateId> {
        if let Some(&id) = self.state_ids.get(&address(state)) {
            return Ok(id);
        }
        let key = StateKey::of(state);
        if let Some(&id) = self.state_values.get(&key) {
            self.state_aliases.push(Arc::clone(state));
            self.state_ids.insert(address(state), id);
            return Ok(id);
        }
        bytes.charge_items::<Composite>(1)?;
        let id = StateId(index(self.states.len())?);
        self.states.push(Arc::clone(state));
        self.state_ids.insert(address(state), id);
        self.state_values.insert(key, id);
        Ok(id)
    }

    pub(super) fn paint(&mut self, paint: &Arc<Paint>, bytes: &mut Meter) -> Result<PaintId> {
        let address = address(paint);
        if let Some(&(_, id)) = self
            .recent_paints
            .iter()
            .flatten()
            .find(|(recent, _)| *recent == address)
        {
            return Ok(id);
        }
        let id = self.paint_by_value(paint, bytes)?;
        self.recent_paints = [Some((address, id)), self.recent_paints[0]];
        Ok(id)
    }

    fn paint_by_value(&mut self, paint: &Arc<Paint>, bytes: &mut Meter) -> Result<PaintId> {
        if let Some(&id) = self.paint_ids.get(&address(paint)) {
            return Ok(id);
        }
        let key = match paint.as_ref() {
            Paint::Solid(color) => Some((
                address(&color.space),
                color
                    .components
                    .iter()
                    .map(|c| c.to_bits())
                    .collect::<Vec<_>>(),
            )),
            Paint::Shading(_) | Paint::Tiling(_) => None,
        };
        if let Some(&id) = key.as_ref().and_then(|key| self.paint_values.get(key)) {
            self.paint_aliases.push(Arc::clone(paint));
            self.paint_ids.insert(address(paint), id);
            return Ok(id);
        }
        bytes.charge_items::<Paint>(1)?;
        let id = PaintId(index(self.paints.len())?);
        self.paints.push(Arc::clone(paint));
        self.paint_ids.insert(address(paint), id);
        if let Some(key) = key {
            self.paint_values.insert(key, id);
        }
        Ok(id)
    }

    pub(super) fn pen(&mut self, pen: &Arc<Pen>, bytes: &mut Meter) -> Result<PenId> {
        if let Some(&id) = self.pen_ids.get(&address(pen)) {
            return Ok(id);
        }
        bytes.charge_items::<Pen>(1)?;
        bytes.charge_items::<f64>(pen.stroke.dash_array.len())?;
        let id = PenId(index(self.pens.len())?);
        self.pens.push(Arc::clone(pen));
        self.pen_ids.insert(address(pen), id);
        Ok(id)
    }

    /// Interns `clip` and the chain above it.
    pub(super) fn clip(&mut self, clip: &Arc<Clip>, bytes: &mut Meter) -> Result<ClipId> {
        let address = address(clip);
        if let Some((recent, id)) = self.recent_clip
            && recent == address
        {
            return Ok(id);
        }
        let id = self.clip_chain(clip, bytes)?;
        self.recent_clip = Some((address, id));
        Ok(id)
    }

    fn clip_chain(&mut self, clip: &Arc<Clip>, bytes: &mut Meter) -> Result<ClipId> {
        if let Some(&id) = self.clip_ids.get(&address(clip)) {
            return Ok(id);
        }
        // Walk up to the first interned ancestor, then intern downwards.
        let mut chain = vec![clip];
        let mut parent = None;
        let mut next = clip.parent();
        while let Some(node) = next {
            if let Some(&id) = self.clip_ids.get(&address(node)) {
                parent = Some(id);
                break;
            }
            chain.push(node);
            next = node.parent();
        }
        for node in chain.into_iter().rev() {
            bytes.charge_items::<ClipNode>(1)?;
            bytes.charge_items::<pagura_raster::Segment>(node.path().segments().len())?;
            let id = ClipId(index(self.clips.len())?);
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
