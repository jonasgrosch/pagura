//! Interning of the shared values marks carry, by `Arc` identity: each
//! value the interpreter shares becomes one table entry. The tables hold
//! the `Arc`s, so an identity is never reused while it is interned.

use crate::model::{Clip, Composite, Paint, Pen};
use crate::scene::{ClipId, ClipNode, PaintId, PenId, StateId};
use pagura_core::{Error, Meter, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// The interned tables of a recording.
#[derive(Debug, Default)]
pub(super) struct Interner {
    clips: Vec<ClipNode>,
    clip_ids: HashMap<usize, ClipId>,
    /// The interned clips, kept so their addresses stay theirs.
    clip_arcs: Vec<Arc<Clip>>,
    states: Vec<Arc<Composite>>,
    state_ids: HashMap<usize, StateId>,
    paints: Vec<Arc<Paint>>,
    paint_ids: HashMap<usize, PaintId>,
    pens: Vec<Arc<Pen>>,
    pen_ids: HashMap<usize, PenId>,
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
        if let Some(&id) = self.state_ids.get(&address(state)) {
            return Ok(id);
        }
        bytes.charge_items::<Composite>(1)?;
        let id = StateId(index(self.states.len())?);
        self.states.push(Arc::clone(state));
        self.state_ids.insert(address(state), id);
        Ok(id)
    }

    pub(super) fn paint(&mut self, paint: &Arc<Paint>, bytes: &mut Meter) -> Result<PaintId> {
        if let Some(&id) = self.paint_ids.get(&address(paint)) {
            return Ok(id);
        }
        bytes.charge_items::<Paint>(1)?;
        let id = PaintId(index(self.paints.len())?);
        self.paints.push(Arc::clone(paint));
        self.paint_ids.insert(address(paint), id);
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
