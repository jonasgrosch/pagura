//! Activation (ADR 0034 §4): each group's items enter its active list at
//! their first band and leave after their last, and the list stays in
//! source order, so a band costs the items it meets.

use crate::plan::{GroupPlan, Item};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};

/// One group's active items.
#[derive(Debug, Default)]
pub(crate) struct Active {
    /// Positions in the group's items, ascending.
    list: Vec<u32>,
    /// The next position in the group's activation order.
    next: usize,
    /// The band the list is for.
    band: Option<u32>,
}

impl Active {
    /// An empty list with room for `items` positions.
    pub(crate) fn with_capacity(items: usize) -> Result<Self> {
        let mut list = Vec::new();
        list.try_reserve_exact(items)
            .map_err(|_| Error::limit("render buffer allocation failed"))?;
        Ok(Self {
            list,
            next: 0,
            band: None,
        })
    }

    /// Bytes retained.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.list.capacity() * size_of::<u32>()
    }

    /// Forgets every band.
    pub(crate) fn reset(&mut self) {
        self.list.clear();
        self.next = 0;
        self.band = None;
    }

    /// The position at `index` of the list.
    pub(crate) fn get(&self, index: usize) -> Option<u32> {
        self.list.get(index).copied()
    }

    /// Brings the list to `band`: retires items whose last band is above
    /// it and activates items whose first band it has reached.
    pub(crate) fn advance(&mut self, group: &GroupPlan, items: &[Item], band: u32) {
        if self.band == Some(band) {
            return;
        }
        if self.band.is_some_and(|previous| previous > band) {
            self.reset();
        }
        let bands = |position: u32| {
            group
                .items
                .get(u32_to_usize(position))
                .and_then(|&id| items.get(u32_to_usize(id)))
                .map_or([u32::MAX, 0], |item| item.bands)
        };
        self.list.retain(|&position| bands(position)[1] >= band);
        let before = self.list.len();
        while let Some(&position) = group.order.get(self.next) {
            let [first, last] = bands(position);
            if first > band {
                break;
            }
            self.next += 1;
            if last >= band {
                self.list.push(position);
            }
        }
        if self.list.len() != before {
            self.list.sort_unstable();
        }
        self.band = Some(band);
    }
}
