//! Private preparation-only ownership of unexecuted opacity queries.

use super::roots::{self, SourcePlan};
use super::{Entry, HeldTiles, Identity};
use crate::source::Source;
use crate::source::image::column_index::{ColumnIndex, Layout};
use crate::source::image::geometry::Geometry;
use crate::source::image::mask::{DecodedMask, MaskSampling};
use crate::source::image::{Orientation, PaintContext};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Result};
use pagura_image::{DecodedTiles, ImageSource};
use std::num::NonZeroU32;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerToken(NonZeroU32);

impl OwnerToken {
    pub(super) fn of(index: u32) -> Result<Self> {
        NonZeroU32::new(
            index
                .checked_add(1)
                .ok_or(Error::limit("tiled owner count overflow"))?,
        )
        .map(Self)
        .ok_or(Error::limit("tiled owner count overflow"))
    }

    pub(super) fn index(self) -> usize {
        u32_to_usize(self.0.get() - 1)
    }
}

/// Only these queries have not run. Parent, decode, coefficient, planning
/// and construction work cannot enter the cancellable component.
#[derive(Debug, Default)]
pub(super) struct ReservedMaskQueries {
    old: usize,
    indexed: usize,
}

#[derive(Debug)]
pub(crate) struct TiledAdmission<'a> {
    pub(crate) owners: &'a mut HeldTiles,
    pub(crate) prior: Option<&'a mut [Source]>,
}

impl TiledAdmission<'_> {
    pub(in crate::source::image) fn decode(
        &mut self,
        source: &Arc<ImageSource>,
        orientation: Orientation,
        context: &PaintContext<'_>,
        (bytes, work): (&mut Meter, &mut Meter),
    ) -> Result<DecodedMask> {
        let (decoded, owner) =
            self.owners
                .get_metered(source, orientation, context, (bytes, work))?;
        Ok(DecodedMask::Tiles {
            decoded,
            owner: self.prior.as_ref().map(|_| owner),
            index: None,
        })
    }

    /// At the original combined parent/mask admission point, before stencil
    /// recolouring. The old route keeps its checked product and error order.
    pub(in crate::source::image) fn reserve(
        &mut self,
        parent: &Geometry,
        mask: Option<&mut MaskSampling>,
        pixels: usize,
        work: &mut Meter,
        (local, retained): (&mut Meter, &mut Meter),
        context: &PaintContext<'_>,
    ) -> Result<()> {
        let cost = mask.as_deref().map_or(0, |mask| mask.cost(parent));
        let legacy_cost = parent.cost().saturating_add(cost);
        let Some(mask) = mask else {
            return work.charge_product(pixels, legacy_cost);
        };
        let Some((parent_work, old, combined)) = exact_parts(parent.cost(), cost, pixels) else {
            mask.clear_index_owner();
            return work.charge_product(pixels, legacy_cost);
        };
        let Some((token, decoded, query)) = mask.index_candidate(pixels) else {
            return work.charge_product(pixels, legacy_cost);
        };
        let Some(prior) = &mut self.prior else {
            return work.charge_product(pixels, legacy_cost);
        };
        let owners = &mut *self.owners;
        let entry = owners.entry(token, decoded)?;
        if let Some(index) = &entry.index {
            let index = Arc::clone(index);
            let source = owners.source_plan(token, context.limits)?;
            source.admit_work(query)?;
            work.charge(parent_work.checked_add(query).ok_or(work_limit())?)?;
            owners.retain_source_queries(source, query)?;
            let entry = owners.entry_mut(token)?;
            entry.queries.indexed = entry
                .queries
                .indexed
                .checked_add(query)
                .ok_or(work_limit())?;
            mask.attach_index(index);
            return Ok(());
        }
        if work.charge(combined).is_ok() {
            let entry = owners.entry_mut(token)?;
            entry.queries.old = entry.queries.old.checked_add(old).ok_or(work_limit())?;
            // Saturation only disables a potential optimization; it never
            // alters the already admitted exact legacy reservation.
            entry.queries.indexed = entry.queries.indexed.saturating_add(query);
            return Ok(());
        }
        let transition = Transition::new(
            owners,
            token,
            decoded,
            (parent_work, old, query),
            prior.len(),
            context,
        )?;
        let index = transition.build(owners, decoded, prior, (work, local, retained))?;
        let entry = owners.entry_mut(token)?;
        entry.queries.old = 0;
        entry.queries.indexed = transition.queries;
        entry.index = Some(Arc::clone(&index));
        for source in prior.iter_mut() {
            if let Source::Image(image) = source {
                image.attach_index(token, &index);
            }
        }
        mask.attach_index(index);
        Ok(())
    }
}

/// The complete proposal is known before cancellation or any traversal.
#[derive(Debug)]
struct Transition {
    owner: OwnerToken,
    source: SourcePlan,
    layout: Layout,
    cancelled: usize,
    queries: usize,
    parent: usize,
    planning: usize,
}

impl Transition {
    fn new(
        owners: &HeldTiles,
        owner: OwnerToken,
        decoded: &Arc<DecodedTiles>,
        (parent, current_old, current_query): (usize, usize, usize),
        prior: usize,
        context: &PaintContext<'_>,
    ) -> Result<Self> {
        let entry = owners.entry(owner, decoded)?;
        let source = owners.source_plan(owner, context.limits)?;
        let layout = Layout::new(decoded.width(), decoded.height())?;
        let queries = entry
            .queries
            .indexed
            .checked_add(current_query)
            .ok_or(work_limit())?;
        if layout
            .work
            .checked_add(queries)
            .is_none_or(|new| new >= entry.queries.old.saturating_add(current_old))
        {
            return Err(work_limit());
        }
        let owner_passes = if source.root.is_none() {
            owners.entries.len()
        } else {
            0
        };
        // Two owner passes only for a first root; two paint passes for every
        // transition. The fixed setup covers the bounded scalar plan checks.
        let planning = owner_passes
            .checked_add(prior)
            .and_then(|n| n.checked_mul(2 * roots::STEP))
            .and_then(|n| n.checked_add(32))
            .ok_or(work_limit())?;
        let operations = layout.work.checked_add(queries).ok_or(work_limit())?;
        source
            .admit_work(planning.checked_add(operations).ok_or(work_limit())?)
            .map_err(|_| work_limit())?;
        Ok(Self {
            owner,
            source,
            layout,
            cancelled: entry.queries.old,
            queries,
            parent,
            planning,
        })
    }

    fn build(
        &self,
        owners: &mut HeldTiles,
        decoded: &Arc<DecodedTiles>,
        prior: &mut [Source],
        (work, local, retained): (&mut Meter, &mut Meter, &mut Meter),
    ) -> Result<Arc<ColumnIndex>> {
        let operations = self
            .layout
            .work
            .checked_add(self.queries)
            .ok_or(work_limit())?;
        let replacement = operations.checked_add(self.parent).ok_or(work_limit())?;
        let mut prospective = work.clone();
        prospective.replace_reservation(
            self.cancelled,
            replacement.checked_add(self.planning).ok_or(work_limit())?,
        )?;
        // Execute no pass until both caller and root have admitted all its
        // work. Planning stays charged even on later failure.
        work.charge(self.planning)?;
        owners.spend_source_planning(self.source, self.planning)?;
        let mut source = self.source;
        if source.root.is_none() {
            source.bytes = owners.aggregate_source_bytes(self.owner)?;
        }
        source
            .admit_bytes(self.layout.bytes)
            .map_err(|_| work_limit())?;
        let available = source
            .allowance
            .bytes
            .checked_sub(source.bytes)
            .ok_or(work_limit())?
            .min(local.remaining())
            .min(retained.remaining());
        if self.layout.bytes > available {
            return Err(work_limit());
        }
        for source in prior.iter() {
            if let Source::Image(image) = source {
                image.validate_index_owner(self.owner, decoded)?;
            }
        }
        // Only prior unexecuted old queries are replaced. The already
        // charged planning allowance is deliberately absent here.
        work.replace_reservation(self.cancelled, replacement)?;
        let mut allocation = Meter::new(available, "render memory budget exceeded");
        let index = Arc::new(ColumnIndex::new(decoded, &mut allocation)?);
        let actual = index.bytes();
        source.admit_bytes(actual)?;
        local.charge(actual)?;
        retained.charge(actual)?;
        owners.retain_source_index(self.owner, source, (actual, operations), self.planning)?;
        Ok(index)
    }
}

fn exact_parts(parent: usize, mask: usize, pixels: usize) -> Option<(usize, usize, usize)> {
    let total = parent.checked_add(mask)?;
    Some((
        parent.checked_mul(pixels)?,
        mask.checked_mul(pixels)?,
        total.checked_mul(pixels)?,
    ))
}

fn work_limit() -> Error {
    Error::limit("render work budget exceeded")
}

impl HeldTiles {
    fn entry(&self, token: OwnerToken, decoded: &Arc<DecodedTiles>) -> Result<&Entry> {
        let entry = self
            .entries
            .get(token.index())
            .ok_or(Error::invalid_argument("invalid tiled owner token"))?;
        let identity = Identity {
            source: Arc::as_ptr(&entry.key.source).addr(),
            orientation: entry.key.orientation,
            height: entry.key.height,
        };
        if self.ids.get(&identity).copied() != Some(token.0.get() - 1)
            || !Arc::ptr_eq(decoded, &entry.value)
        {
            return Err(Error::invalid_argument("tiled owner identity mismatch"));
        }
        Ok(entry)
    }

    fn entry_mut(&mut self, token: OwnerToken) -> Result<&mut Entry> {
        self.entries
            .get_mut(token.index())
            .ok_or(Error::invalid_argument("invalid tiled owner token"))
    }
}
