//! Actual key/root ownership and checked per-request admission.

use super::*;
use pagura_color::{Process, Target};
use pagura_core::ObjectId;
use pagura_document::{Document, OpenOptions};
use pagura_interpret::Composite;
use pagura_raster::{DeviceRect, Transform};
use pagura_testkit::fixture;

fn source() -> Arc<ImageSource> {
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                pixels: 10,
                ..pagura_core::Limits::default()
            },
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

#[test]
fn supported_layouts_include_both_local_and_persistent_owner_metadata() {
    #[cfg(target_pointer_width = "64")]
    assert_eq!(
        (
            size_of::<pagura_image::TileLayout>(),
            size_of::<Key>(),
            size_of::<Identity>(),
            size_of::<Entry>(),
            size_of::<HeldTiles>(),
            publication_bytes()
        ),
        (40, 16, 16, 104, 64, 40),
    );
    #[cfg(target_arch = "wasm32")]
    assert_eq!(
        (
            size_of::<pagura_image::TileLayout>(),
            size_of::<Key>(),
            size_of::<Identity>(),
            size_of::<Entry>(),
            size_of::<HeldTiles>(),
            publication_bytes()
        ),
        (20, 12, 12, 56, 32, 24),
    );
}

#[test]
fn local_reuse_and_persistent_keys_keep_exact_source_identity_and_lifetime() {
    let store = Store::new(64 * 1024 * 1024);
    let limits = pagura_core::Limits::default();
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
    let source = source();
    let weak = Arc::downgrade(&source);
    let mut owners = HeldTiles::default();
    let mut bytes = Meter::new(1 << 20, "test ownership budget");
    let first = owners
        .get(&source, Orientation::Rows, &context, &mut bytes)
        .unwrap();
    let again = owners
        .get(&source, Orientation::Rows, &context, &mut bytes)
        .unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    let equal_root = Arc::new(source.as_ref().clone());
    assert_eq!(equal_root.id(), source.id());
    let other = owners
        .get(&equal_root, Orientation::Rows, &context, &mut bytes)
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &other));
    assert_eq!(owners.entries.len(), 2);
    assert_eq!(
        store.retained_bytes(),
        0,
        "pending owners are not published"
    );
    let low = pagura_core::Limits {
        pixels: 9,
        ..limits
    };
    let refused = PaintContext {
        limits: &low,
        ..context
    };
    let error = owners
        .get(&source, Orientation::Rows, &refused, &mut bytes)
        .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(owners.entries.len(), 2);
    drop(source);
    assert!(weak.upgrade().is_some());
    owners.publish(&store);
    assert!(
        store.retained_bytes() > first.retained_bytes() + other.retained_bytes(),
        "Store pays the key-held source/root/wrapper owners too"
    );
    let temporary = owners.index_bytes(&bytes).unwrap();
    assert!(temporary > 0 && temporary < bytes.used());
    drop(owners);
    assert!(
        weak.upgrade().is_some(),
        "persistent key holds the source root"
    );
    store.clear();
    assert!(weak.upgrade().is_none());
    assert_eq!(
        first.get(2, 1),
        Some(1),
        "decoded allocation outlives source"
    );
    assert_eq!(store.retained_bytes(), 0);
}

#[test]
fn cumulative_unique_owner_reservation_fails_before_new_value_or_cache_mutation() {
    let store = Store::new(64 * 1024 * 1024);
    let limits = pagura_core::Limits::default();
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
    let first = source();
    let second = Arc::new(first.as_ref().clone());
    let third = Arc::new(first.as_ref().clone());
    let mut owners = HeldTiles::default();
    let mut bytes = Meter::new(1 << 20, "test ownership budget");
    drop(
        owners
            .get(&first, Orientation::Rows, &context, &mut bytes)
            .unwrap(),
    );
    drop(
        owners
            .get(&second, Orientation::Rows, &context, &mut bytes)
            .unwrap(),
    );
    assert_eq!(owners.entries.len(), 2);
    let used = bytes.used();
    let mut low = Meter::new(used + 1, "test ownership budget");
    low.charge(used).unwrap();
    let error = owners
        .get(&third, Orientation::Rows, &context, &mut low)
        .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(owners.entries.len(), 2);
    assert_eq!(owners.ids.len(), 2);
    assert_eq!(low.used(), used);
    assert_eq!(store.retained_bytes(), 0);
    drop(owners);
    assert_eq!(store.retained_bytes(), 0);
}

#[test]
fn legacy_work_product_overflow_is_a_refusal_before_any_sampling() {
    use crate::source::image::geometry::Geometry;
    let store = Store::new(0);
    let limits = pagura_core::Limits {
        render_work: usize::MAX,
        ..pagura_core::Limits::default()
    };
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
    let geometry = Geometry::new((10, 7), false, Transform::IDENTITY).unwrap();
    assert!(geometry.cost() > 1);
    let mut owners = HeldTiles::default();
    let mut admission = TiledAdmission {
        owners: &mut owners,
        prior: None,
    };
    let mut work = Meter::new(usize::MAX, "render work budget exceeded");
    let mut local = Meter::new(1 << 20, "test local bytes");
    let mut retained = Meter::new(1 << 20, "test retained bytes");
    let error = admission
        .reserve(
            &geometry,
            None,
            usize::MAX,
            &mut work,
            (&mut local, &mut retained),
            &context,
        )
        .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!((work.used(), local.used(), retained.used()), (0, 0, 0));
    assert_eq!(store.retained_bytes(), 0);
}

fn source_with_render_limit(render_bytes: usize) -> Arc<ImageSource> {
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                pixels: 10,
                render_bytes,
                ..pagura_core::Limits::default()
            },
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

#[test]
fn new_aliases_respect_the_original_aggregate_index_owner_byte_allowance() {
    let limits = pagura_core::Limits::default();
    let probe = source();
    let rows = probe
        .decode_tiles(pagura_image::Orientation::Rows, &limits)
        .unwrap();
    let transposed = probe
        .decode_tiles(pagura_image::Orientation::Transposed, &limits)
        .unwrap();
    let index_layout =
        super::super::column_index::Layout::new(rows.width(), rows.height()).unwrap();
    let cap =
        rows.retained_bytes() + ARC + index_layout.bytes + transposed.retained_bytes() + ARC - 1;
    let source = source_with_render_limit(cap);
    assert_eq!(
        source
            .tile_layout(pagura_image::Orientation::Rows, &limits)
            .unwrap()
            .index_byte_limit(),
        cap
    );
    let store = Store::new(0);
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
    let mut bytes = Meter::new(1 << 20, "test owner bytes");
    let mut owners = HeldTiles::default();
    let value = owners
        .get(&source, Orientation::Rows, &context, &mut bytes)
        .unwrap();
    let mut index_bytes = Meter::new(1 << 20, "test index bytes");
    let index = Arc::new(ColumnIndex::new(&value, &mut index_bytes).unwrap());
    assert_eq!(index.bytes(), index_layout.bytes);
    bytes.charge(index.bytes()).unwrap();
    // The new inline ledger must be populated as in a completed index
    // transition; limits, source bytes and the expected refusal are unchanged.
    let owner = OwnerToken::of(0).unwrap();
    let mut plan = owners.source_plan(owner, &limits).unwrap();
    plan.bytes = owners.aggregate_source_bytes(owner).unwrap();
    owners
        .retain_source_index(owner, plan, (index.bytes(), index_layout.work), 0)
        .unwrap();
    owners.entries[0].index = Some(index);
    let before = bytes.used();
    let error = owners
        .get(&source, Orientation::Transposed, &context, &mut bytes)
        .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(owners.entries.len(), 1);
    assert_eq!(bytes.used(), before);
    assert_eq!(store.retained_bytes(), 0);
}
