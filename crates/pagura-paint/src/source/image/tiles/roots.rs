//! Bounded source-root accounting in pre-admitted owner slots.
//!
//! Each root's existing Entry::key holds its strong SourceArc. Tokens are
//! stable append-only indices, confined to the current preparation pool.

use super::reservations::OwnerToken;
use super::{ARC, Entry, HeldTiles, Identity, publication_bytes};
use crate::source::image::PaintContext;
use pagura_core::{Error, Limits, Meter, Result};
use pagura_image::{ImageSource, TileLayout};
use std::sync::Arc;

pub(super) const STEP: usize = 16;

#[derive(Debug, Clone, Copy)]
pub(super) struct Allowance {
    pub(super) bytes: usize,
    pub(super) work: usize,
}

impl Allowance {
    pub(super) fn of(layout: &TileLayout) -> Self {
        Self {
            bytes: layout.index_byte_limit(),
            work: layout.index_work_limit(),
        }
    }

    fn intersection(self, limits: &Limits) -> Self {
        Self {
            bytes: self.bytes.min(limits.render_bytes),
            work: self.work.min(limits.render_work),
        }
    }

    fn restrict(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.min(other.bytes),
            work: self.work.min(other.work),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RootToken(OwnerToken);

#[derive(Debug)]
pub(super) enum SourceLedger {
    Unindexed(Allowance),
    Alias(RootToken),
    Root(Root),
}

#[derive(Debug)]
pub(super) struct Root {
    bytes: usize,
    work: usize,
    allowance: Allowance,
    next: Option<RootToken>,
}

/// An immutable proposal; only explicit private operations commit its costs.
#[derive(Debug, Clone, Copy)]
pub(super) struct SourcePlan {
    pub(super) root: Option<RootToken>,
    pub(super) bytes: usize,
    pub(super) work: usize,
    pub(super) allowance: Allowance,
}

impl SourcePlan {
    pub(super) fn admit_work(self, additional: usize) -> Result<()> {
        let mut work = Meter::new(
            self.allowance.work,
            "tiled column index source work budget exceeded",
        );
        work.charge(self.work)?;
        work.charge(additional)
    }

    pub(super) fn admit_bytes(self, additional: usize) -> Result<()> {
        let mut bytes = Meter::new(
            self.allowance.bytes,
            "tiled column index source memory budget exceeded",
        );
        bytes.charge(self.bytes)?;
        bytes.charge(additional)
    }
}

/// Per admitted owner, not per decoded-pointer identity. Conservatively count
/// duplicate physical data across different layout keys; never dedup by scans.
pub(super) fn owner_bytes(decoded: usize) -> Result<usize> {
    decoded
        .checked_add(ARC)
        .and_then(|n| n.checked_add(publication_bytes()))
        .and_then(|n| n.checked_add(size_of::<Entry>()))
        .and_then(|n| n.checked_add(size_of::<(Identity, u32)>()))
        .ok_or(Error::limit("tiled source owner byte count overflow"))
}

impl HeldTiles {
    /// Only activated indexed roots are visited. Before each comparison, the
    /// caller and that visited root pay for their own step. No unrelated
    /// unindexed source receives a new captured query cap.
    pub(super) fn find_source_root(
        &mut self,
        source: &Arc<ImageSource>,
        work: &mut Meter,
        limits: &Limits,
    ) -> Result<Option<RootToken>> {
        let mut next = self.root_head;
        let mut remaining = self.entries.len();
        while let Some(token) = next {
            remaining = remaining
                .checked_sub(1)
                .ok_or(Error::invalid_argument("cyclic tiled source root list"))?;
            work.charge(STEP)?;
            let entry = self
                .entries
                .get_mut(token.0.index())
                .ok_or(Error::invalid_argument("invalid tiled source root token"))?;
            let SourceLedger::Root(root) = &mut entry.ledger else {
                return Err(Error::invalid_argument("tiled source token is not a root"));
            };
            let allowance = root.allowance.intersection(limits);
            let proposed = root
                .work
                .checked_add(STEP)
                .filter(|&n| n <= allowance.work)
                .ok_or(Error::limit(
                    "tiled column index source work budget exceeded",
                ))?;
            root.work = proposed;
            if Arc::ptr_eq(source, &entry.key.source) {
                return Ok(Some(token));
            }
            next = root.next;
        }
        Ok(None)
    }

    pub(super) fn source_plan(&self, owner: OwnerToken, limits: &Limits) -> Result<SourcePlan> {
        let entry = self
            .entries
            .get(owner.index())
            .ok_or(Error::invalid_argument("invalid tiled owner token"))?;
        let token = match &entry.ledger {
            SourceLedger::Unindexed(allowance) => {
                return Ok(SourcePlan {
                    root: None,
                    bytes: 0,
                    work: 0,
                    allowance: allowance.intersection(limits),
                });
            }
            SourceLedger::Alias(token) => *token,
            SourceLedger::Root(_) => RootToken(owner),
        };
        let root_entry = self
            .entries
            .get(token.0.index())
            .ok_or(Error::invalid_argument("invalid tiled source root token"))?;
        if !Arc::ptr_eq(&entry.key.source, &root_entry.key.source) {
            return Err(Error::invalid_argument(
                "tiled source root identity mismatch",
            ));
        }
        let SourceLedger::Root(root) = &root_entry.ledger else {
            return Err(Error::invalid_argument("tiled source token is not a root"));
        };
        Ok(SourcePlan {
            root: Some(token),
            bytes: root.bytes,
            work: root.work,
            allowance: root.allowance.intersection(limits),
        })
    }

    pub(super) fn validate_source_hit(
        &self,
        owner: OwnerToken,
        layout: &TileLayout,
        limits: &Limits,
    ) -> Result<()> {
        let mut plan = self.source_plan(owner, limits)?;
        if plan.root.is_none() {
            return Ok(());
        }
        plan.allowance = plan.allowance.restrict(Allowance::of(layout));
        plan.admit_bytes(0)?;
        plan.admit_work(0)
    }

    /// Before new owner/table/decode allocations, reserve the complete
    /// collection peak. Retained counter updates happen only after success.
    pub(super) fn admit_source_alias(
        &self,
        token: RootToken,
        layout: &TileLayout,
        context: &PaintContext<'_>,
    ) -> Result<()> {
        let mut plan = self.source_plan(token.0, context.limits)?;
        plan.allowance = plan.allowance.restrict(Allowance::of(layout));
        plan.admit_work(0)?;
        plan.admit_bytes(owner_bytes(layout.new_peak_bytes())?)
    }

    pub(super) fn retain_source_alias(&mut self, token: RootToken, retained: usize) -> Result<()> {
        let root = self.root_mut(token)?;
        root.bytes = root
            .bytes
            .checked_add(owner_bytes(retained)?)
            .filter(|&n| n <= root.allowance.bytes)
            .ok_or(Error::limit(
                "tiled column index source memory budget exceeded",
            ))?;
        Ok(())
    }

    /// Requires the separately admitted first-root owner-entry pass. This
    /// sums one conservative charge per key, even for shared decoded Arcs.
    pub(super) fn aggregate_source_bytes(&self, owner: OwnerToken) -> Result<usize> {
        let source = &self
            .entries
            .get(owner.index())
            .ok_or(Error::invalid_argument("invalid tiled owner token"))?
            .key
            .source;
        let mut bytes = 0_usize;
        for entry in &self.entries {
            if !Arc::ptr_eq(source, &entry.key.source) {
                continue;
            }
            if !matches!(entry.ledger, SourceLedger::Unindexed(_)) || entry.index.is_some() {
                return Err(Error::invalid_argument(
                    "unindexed source has an active alias",
                ));
            }
            bytes = bytes
                .checked_add(owner_bytes(entry.value.retained_bytes())?)
                .ok_or(Error::limit("tiled source owner byte count overflow"))?;
        }
        Ok(bytes)
    }

    /// Charge executed planning to an existing root before the first pass.
    /// A first root carries these admitted costs in its private SourcePlan
    /// until construction commits; no public/private continuation follows an
    /// error from this preparation.
    pub(super) fn spend_source_planning(&mut self, plan: SourcePlan, units: usize) -> Result<()> {
        plan.admit_work(units)?;
        if let Some(token) = plan.root {
            let root = self.root_mut(token)?;
            root.work = root.work.checked_add(units).ok_or(Error::limit(
                "tiled column index source work budget exceeded",
            ))?;
        }
        Ok(())
    }

    /// All work/bytes are pre-admitted. A first root's second owner pass is
    /// separately charged; every existing alias receives the stable root ID.
    pub(super) fn retain_source_index(
        &mut self,
        owner: OwnerToken,
        plan: SourcePlan,
        (bytes, operations): (usize, usize),
        planning: usize,
    ) -> Result<()> {
        plan.admit_bytes(bytes)?;
        plan.admit_work(planning.checked_add(operations).ok_or(Error::limit(
            "tiled column index source work budget exceeded",
        ))?)?;
        if let Some(token) = plan.root {
            let root = self.root_mut(token)?;
            root.bytes = root
                .bytes
                .checked_add(bytes)
                .ok_or(Error::limit("tiled source owner byte count overflow"))?;
            // Planning was already spent before execution and is not added twice.
            root.work = root.work.checked_add(operations).ok_or(Error::limit(
                "tiled column index source work budget exceeded",
            ))?;
            return Ok(());
        }
        let token = RootToken(owner);
        let entry = self
            .entries
            .get_mut(owner.index())
            .ok_or(Error::invalid_argument("invalid tiled owner token"))?;
        let source = Arc::clone(&entry.key.source);
        entry.ledger = SourceLedger::Root(Root {
            bytes: plan
                .bytes
                .checked_add(bytes)
                .ok_or(Error::limit("tiled source owner byte count overflow"))?,
            work: planning.checked_add(operations).ok_or(Error::limit(
                "tiled column index source work budget exceeded",
            ))?,
            allowance: plan.allowance,
            next: self.root_head,
        });
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if index != owner.index() && Arc::ptr_eq(&source, &entry.key.source) {
                entry.ledger = SourceLedger::Alias(token);
            }
        }
        self.root_head = Some(token);
        Ok(())
    }

    pub(super) fn retain_source_queries(&mut self, plan: SourcePlan, units: usize) -> Result<()> {
        plan.admit_work(units)?;
        let token = plan
            .root
            .ok_or(Error::invalid_argument("indexed owner has no source root"))?;
        let root = self.root_mut(token)?;
        root.work = root.work.checked_add(units).ok_or(Error::limit(
            "tiled column index source work budget exceeded",
        ))?;
        Ok(())
    }

    fn root_mut(&mut self, token: RootToken) -> Result<&mut Root> {
        let entry = self
            .entries
            .get_mut(token.0.index())
            .ok_or(Error::invalid_argument("invalid tiled source root token"))?;
        let SourceLedger::Root(root) = &mut entry.ledger else {
            return Err(Error::invalid_argument("tiled source token is not a root"));
        };
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::super::reservations::OwnerToken;
    use super::*;
    use crate::source::image::{
        Orientation,
        column_index::{ColumnIndex, Layout},
    };
    use pagura_color::{Process, Target};
    use pagura_core::ObjectId;
    use pagura_document::{Document, OpenOptions};
    use pagura_image::DecodedTiles;
    use pagura_interpret::Composite;
    use pagura_raster::{DeviceRect, Transform};
    use pagura_testkit::fixture;

    fn source(limits: &Limits) -> Arc<ImageSource> {
        let document = Document::open(
            fixture("packed_mask_tiles", "small-flate.pdf"),
            &OpenOptions {
                limits: *limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        Arc::new(
            ImageSource::from_xobject_with_tiling(
                &document,
                document.object(ObjectId::new(6, 0)).unwrap(),
            )
            .unwrap(),
        )
    }

    /// Valid physical index and ledger setup; no sampling or publication.
    fn activate(
        owners: &mut HeldTiles,
        owner: OwnerToken,
        (bytes, work): (&mut Meter, &mut Meter),
        limits: &Limits,
    ) -> Arc<ColumnIndex> {
        let decoded = Arc::clone(&owners.entries[owner.index()].value);
        let layout = Layout::new(decoded.width(), decoded.height()).unwrap();
        let mut plan = owners.source_plan(owner, limits).unwrap();
        let planning = owners.entries.len() * 2 * STEP;
        plan.admit_work(planning + layout.work).unwrap();
        work.charge(planning + layout.work).unwrap();
        owners.spend_source_planning(plan, planning).unwrap();
        plan.bytes = owners.aggregate_source_bytes(owner).unwrap();
        plan.admit_bytes(layout.bytes).unwrap();
        let index = Arc::new(ColumnIndex::new(&decoded, bytes).unwrap());
        owners
            .retain_source_index(owner, plan, (index.bytes(), layout.work), planning)
            .unwrap();
        owners.entries[owner.index()].index = Some(Arc::clone(&index));
        index
    }

    #[test]
    fn root_steps_charge_each_visited_root_and_preserve_original_work_caps() {
        let cap = Layout::new(10, 7).unwrap().work + 2 * STEP + 3 * STEP;
        let first = source(&Limits {
            pixels: 10,
            render_work: cap,
            ..Limits::default()
        });
        let second = source(&Limits {
            pixels: 10,
            ..Limits::default()
        });
        let unrelated = source(&Limits {
            pixels: 10,
            render_work: 0,
            ..Limits::default()
        });
        let limits = Limits::default();
        let store = pagura_core::Store::new(0);
        let context = PaintContext {
            transform: Transform::IDENTITY,
            target: &Target::new(Process::Rgb),
            composite: &Composite::default(),
            store: &store,
            limits: &limits,
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
        };
        let mut owners = HeldTiles::default();
        let mut bytes = Meter::new(1 << 20, "test bytes");
        let mut work = Meter::new(2_000_000_000, "test work");
        let (_, a) = owners
            .get_metered(&first, Orientation::Rows, &context, (&mut bytes, &mut work))
            .unwrap();
        drop(activate(&mut owners, a, (&mut bytes, &mut work), &limits));
        let (_, b) = owners
            .get_metered(
                &second,
                Orientation::Rows,
                &context,
                (&mut bytes, &mut work),
            )
            .unwrap();
        drop(activate(&mut owners, b, (&mut bytes, &mut work), &limits));
        let a0 = owners.source_plan(a, &limits).unwrap().work;
        let b0 = owners.source_plan(b, &limits).unwrap().work;
        for _ in 0..2 {
            let before = work.used();
            assert_eq!(
                owners
                    .find_source_root(&unrelated, &mut work, &limits)
                    .unwrap(),
                None
            );
            assert_eq!(work.used() - before, 2 * STEP);
        }
        assert_eq!(owners.source_plan(a, &limits).unwrap().work, a0 + 2 * STEP);
        assert_eq!(owners.source_plan(b, &limits).unwrap().work, b0 + 2 * STEP);
        assert_eq!(owners.source_plan(a, &limits).unwrap().work, cap);
        let before = work.used();
        let error = owners
            .find_source_root(&unrelated, &mut work, &limits)
            .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(work.used() - before, 2 * STEP);
        assert_eq!(owners.source_plan(a, &limits).unwrap().work, cap);
        assert_eq!(owners.entries.len(), 2);
        assert_eq!(store.retained_bytes(), 0);
    }

    #[test]
    fn shared_decoded_arcs_do_not_merge_distinct_source_roots_or_sampler_tokens() {
        let first = source(&Limits {
            pixels: 10,
            ..Limits::default()
        });
        let second = Arc::new(first.as_ref().clone()); // Same ImageId and bytes.
        let limits = Limits::default();
        let store = pagura_core::Store::new(0);
        let context = PaintContext {
            transform: Transform::IDENTITY,
            target: &Target::new(Process::Rgb),
            composite: &Composite::default(),
            store: &store,
            limits: &limits,
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
        };
        let mut owners = HeldTiles::default();
        let mut bytes = Meter::new(1 << 20, "test bytes");
        let mut work = Meter::new(2_000_000_000, "test work");
        let (decoded, a) = owners
            .get_metered(&first, Orientation::Rows, &context, (&mut bytes, &mut work))
            .unwrap();
        let (_, b) = owners
            .get_metered(
                &second,
                Orientation::Rows,
                &context,
                (&mut bytes, &mut work),
            )
            .unwrap();
        owners.entries[b.index()].value = Arc::clone(&decoded); // Synthetic future interning.
        let index = activate(&mut owners, a, (&mut bytes, &mut work), &limits);
        assert!(owners.source_plan(a, &limits).unwrap().root.is_some());
        assert!(owners.source_plan(b, &limits).unwrap().root.is_none());
        let mut sampler = sampler(Arc::clone(&decoded), b);
        sampler.attach_index(a, &index);
        let mask = &sampler
            .prepared
            .as_ref()
            .unwrap()
            .mask
            .as_ref()
            .unwrap()
            .image;
        assert!(matches!(
            mask,
            crate::source::image::mask::DecodedMask::Tiles { index: None, .. }
        ));
        let root_bytes = owners.source_plan(a, &limits).unwrap().bytes;
        assert_eq!(
            root_bytes,
            owner_bytes(decoded.retained_bytes()).unwrap() + index.bytes()
        );
        assert_eq!(store.retained_bytes(), 0);
    }

    fn sampler(decoded: Arc<DecodedTiles>, owner: OwnerToken) -> crate::source::ImageSampler {
        use crate::source::image::{
            Description, ImageBudgets, MaskDescription, Prepared, mask::DecodedMask,
        };
        use pagura_image::{Decoded, DecodedImage, ImageKind, MaskKind, Samples};
        let parent = Arc::new(Decoded::from(DecodedImage {
            width: 1,
            height: 1,
            bits_per_component: 1,
            components: 1,
            samples: Samples::Bytes(vec![0]),
            kind: ImageKind::Stencil { inverted: false },
            decode: None,
            interpolate: false,
            mask: MaskKind::None,
        }));
        let mut bytes = Meter::new(1 << 20, "test sampler bytes");
        let prepared = Prepared::new(
            &Description {
                space: None,
                decode: &[[0.0, 1.0]],
                key: None,
                mask: MaskDescription::Soft {
                    decode: [0.0, 1.0],
                    matte: None,
                },
                alpha_is_shape: false,
            },
            (
                parent,
                Some(DecodedMask::Tiles {
                    decoded,
                    owner: Some(owner),
                    index: None,
                }),
            ),
            (Transform::IDENTITY, (0, 1), Orientation::Rows),
            None,
            None,
            1,
            (
                ImageBudgets {
                    bytes: &mut bytes,
                    work: &mut Meter::new(1 << 20, "test work"),
                    functions: &mut Meter::new(1 << 20, "test functions"),
                },
                None,
            ),
        )
        .unwrap();
        crate::source::ImageSampler::new(Some(Box::new(prepared)), 0)
    }

    #[test]
    fn malformed_private_root_links_refuse_before_unbounded_traversal() {
        let first = source(&Limits {
            pixels: 10,
            ..Limits::default()
        });
        let unrelated = Arc::new(first.as_ref().clone());
        let limits = Limits::default();
        let store = pagura_core::Store::new(0);
        let context = PaintContext {
            transform: Transform::IDENTITY,
            target: &Target::new(Process::Rgb),
            composite: &Composite::default(),
            store: &store,
            limits: &limits,
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
        };
        let mut owners = HeldTiles::default();
        let mut bytes = Meter::new(1 << 20, "test bytes");
        let mut work = Meter::new(2_000_000_000, "test work");
        let (_, owner) = owners
            .get_metered(&first, Orientation::Rows, &context, (&mut bytes, &mut work))
            .unwrap();
        drop(activate(
            &mut owners,
            owner,
            (&mut bytes, &mut work),
            &limits,
        ));
        let token = owners.root_head.unwrap();
        owners.root_mut(token).unwrap().next = Some(token);
        let before = work.used();
        assert_eq!(
            owners
                .find_source_root(&unrelated, &mut work, &limits)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::InvalidArgument
        );
        assert_eq!(work.used() - before, STEP);
        owners.root_head = Some(RootToken(OwnerToken::of(999).unwrap()));
        assert_eq!(
            owners
                .find_source_root(&unrelated, &mut work, &limits)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::InvalidArgument
        );
    }

    #[test]
    fn supported_target_layouts_are_measured_without_a_lint_threshold_exception() {
        let observed = (
            size_of::<Allowance>(),
            size_of::<Root>(),
            size_of::<SourceLedger>(),
            size_of::<Entry>(),
            size_of::<HeldTiles>(),
            size_of::<crate::source::image::mask::MaskSampling>(),
            size_of::<crate::source::image::Prepared>(),
        );
        eprintln!("ROOT_LEDGER_SUPPORTED_LAYOUTS {observed:?}");
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            (observed.0, observed.1, observed.2, observed.3, observed.4),
            (16, 40, 48, 104, 64)
        );
        #[cfg(target_arch = "wasm32")]
        assert_eq!(
            (observed.0, observed.1, observed.2, observed.3, observed.4),
            (8, 20, 24, 56, 32)
        );
    }
}
