//! Held tiled owners for one preparation; persistent misses publish only on success.

use super::{Orientation, PaintContext};
use pagura_core::convert::u32_to_usize;
use pagura_core::hash::FastMap;
use pagura_core::{Built, Error, Meter, Result, Store, StoreKey};
use pagura_image::{DecodedTiles, ImageSource};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[cfg(test)]
mod tests;

const ARC: usize = 2 * size_of::<usize>();

mod reservations;
mod roots;
use super::column_index::ColumnIndex;
pub(super) use reservations::OwnerToken;
use reservations::ReservedMaskQueries;
pub(crate) use reservations::TiledAdmission;
use roots::{Allowance, RootToken, SourceLedger};

/// An address is stable while this typed unmatted-mask key holds its source.
#[derive(Debug, Clone)]
struct Key {
    source: Arc<ImageSource>,
    orientation: Orientation,
    height: u32,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.orientation == other.orientation
            && self.height == other.height
    }
}
impl Eq for Key {}
impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.source).addr().hash(state);
        self.orientation.hash(state);
        self.height.hash(state);
    }
}
impl StoreKey for Key {
    // Store can wrap an already shared collection without copying its vectors.
    type Value = Arc<DecodedTiles>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Identity {
    source: usize,
    orientation: Orientation,
    height: u32,
}

#[derive(Debug)]
enum Origin {
    Stored,
    Pending,
}

#[derive(Debug)]
struct Entry {
    key: Key,
    value: Arc<DecodedTiles>,
    origin: Origin,
    queries: ReservedMaskQueries,
    index: Option<Arc<ColumnIndex>>,
    ledger: SourceLedger,
}

/// First-use ordering is independent of hash iteration and persistent eviction.
#[derive(Debug, Default)]
pub(crate) struct HeldTiles {
    ids: FastMap<Identity, u32>,
    entries: Vec<Entry>,
    root_head: Option<RootToken>,
}

impl HeldTiles {
    #[cfg(test)]
    pub(super) fn get(
        &mut self,
        source: &Arc<ImageSource>,
        orientation: Orientation,
        context: &PaintContext<'_>,
        bytes: &mut Meter,
    ) -> Result<Arc<DecodedTiles>> {
        let mut work = Meter::new(context.limits.render_work, "render work budget exceeded");
        self.get_metered(source, orientation, context, (bytes, &mut work))
            .map(|(value, _)| value)
    }

    pub(super) fn get_metered(
        &mut self,
        source: &Arc<ImageSource>,
        orientation: Orientation,
        context: &PaintContext<'_>,
        (bytes, work): (&mut Meter, &mut Meter),
    ) -> Result<(Arc<DecodedTiles>, OwnerToken)> {
        let requested = match orientation {
            Orientation::Rows => pagura_image::Orientation::Rows,
            Orientation::Transposed => pagura_image::Orientation::Transposed,
        };
        let layout = source.tile_layout(requested, context.limits)?;
        let identity = Identity {
            source: Arc::as_ptr(source).addr(),
            orientation,
            height: layout.height(),
        };
        if let Some(&index) = self.ids.get(&identity) {
            let entry = self
                .entries
                .get(u32_to_usize(index))
                .ok_or(Error::limit("tiled owner table index overflow"))?;
            validate(&entry.value, layout.height(), context)?;
            let owner = OwnerToken::of(index)?;
            self.validate_source_hit(owner, &layout, context.limits)?;
            return Ok((Arc::clone(&entry.value), owner));
        }
        let index = u32::try_from(self.entries.len())
            .map_err(|_| Error::limit("tiled owner count overflow"))?;
        let owner = OwnerToken::of(index)?;
        let root = self.find_source_root(source, work, context.limits)?;
        if let Some(root) = root {
            self.admit_source_alias(root, &layout, context)?;
        }
        bytes.reserve_map_capacity(&mut self.ids, 1)?;
        bytes.reserve_capacity(&mut self.entries, 1)?;
        let key = Key {
            source: Arc::clone(source),
            orientation,
            height: layout.height(),
        };
        // Pay this physical owner once even on persistent hits. Every sampler
        // still pays its own structures; Store state never discounts admission.
        bytes.charge(layout.retained_bytes())?;
        bytes.charge(ARC)?;
        bytes.charge(publication_bytes())?;
        let (value, origin) = if let Some(value) = context.store.get(&key) {
            (Arc::clone(value.as_ref()), Origin::Stored)
        } else {
            let scratch = layout
                .new_peak_bytes()
                .checked_sub(layout.retained_bytes())
                .ok_or(Error::limit("invalid tiled allocation layout"))?;
            bytes.charge(scratch)?;
            let decoded = source.decode_tiles(requested, context.limits)?;
            let value = Arc::new(decoded);
            // All temporary construction storage has been dropped.
            bytes.refund(scratch);
            (value, Origin::Pending)
        };
        if value.retained_bytes() > layout.retained_bytes() {
            return Err(Error::limit("cached tiles exceed validated layout"));
        }
        validate(&value, layout.height(), context)?;
        if let Some(root) = root {
            self.retain_source_alias(root, value.retained_bytes())?;
        }
        self.entries.push(Entry {
            key,
            value: Arc::clone(&value),
            origin,
            queries: ReservedMaskQueries::default(),
            index: None,
            ledger: root.map_or_else(
                || SourceLedger::Unindexed(Allowance::of(&layout)),
                SourceLedger::Alias,
            ),
        });
        self.ids.insert(identity, index);
        Ok((value, owner))
    }

    /// Only optional retention remains. The build closure cannot fail; semantic
    /// decode/preparation failures have already happened before this call.
    pub(crate) fn publish(&self, store: &Store) {
        for entry in &self.entries {
            if !matches!(entry.origin, Origin::Pending) {
                continue;
            }
            let Some(bytes) = cache_bytes(entry) else {
                // An overflowing retention estimate cannot fit a Store budget.
                continue;
            };
            drop(store.get_or_build(entry.key.clone(), || {
                Ok(Built {
                    value: Arc::clone(&entry.value),
                    bytes,
                })
            }));
        }
    }

    /// Slots and their inline source/value handles are temporary. Payloads and
    /// the conservative publication allowance stay charged in PreparedScene.
    pub(crate) fn index_bytes(&self, bytes: &Meter) -> Result<usize> {
        let entries = self
            .entries
            .capacity()
            .checked_mul(size_of::<Entry>())
            .ok_or(Error::limit("tiled owner table byte count overflow"))?;
        bytes
            .map_bytes(&self.ids)?
            .checked_add(entries)
            .ok_or(Error::limit("tiled owner table byte count overflow"))
    }
}

fn publication_bytes() -> usize {
    size_of::<Arc<DecodedTiles>>() + ARC + size_of::<Key>()
}

fn cache_bytes(entry: &Entry) -> Option<usize> {
    entry
        .key
        .source
        .retained_bytes()
        .checked_add(3 * ARC)?
        .checked_add(entry.value.retained_bytes())?
        .checked_add(ARC)?
        .checked_add(publication_bytes())
}

fn validate(value: &DecodedTiles, height: u32, context: &PaintContext<'_>) -> Result<()> {
    Meter::new(
        context.limits.decoded_bytes,
        "tiled image output byte budget exceeded",
    )
    .charge(value.retained_bytes())?;
    for tile in value.tiles() {
        Meter::new(context.limits.pixels, "image sample budget exceeded").charge_product(
            u32_to_usize(tile.image().width),
            u32_to_usize(tile.image().height),
        )?;
        if tile.image().height > height {
            return Err(Error::limit(
                "cached tiled surface exceeds validated height",
            ));
        }
    }
    Ok(())
}
