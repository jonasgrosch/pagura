#![forbid(unsafe_code)]
//! Public facade controls against hand-authored source dictionaries and bytes.
use pagura::files::{CollectionFieldKind, CollectionValue, CollectionView, FilePayload, FileScope};
use pagura::images::ImageTransferMode;
use pagura::metadata::MetadataOwner;
use pagura::{Document, ErrorKind, Limits, ObjectId, OpenOptions, Recovery};
use pagura_testkit::fixture;

fn open(name: &str) -> pagura::Result<Document> {
    Document::open(
        fixture("document_services", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
}
fn id(number: u32) -> ObjectId {
    ObjectId::new(number, 0)
}

#[test]
fn exact_scopes_and_duplicate_related_names_select_independent_bytes() {
    let document = open("complete").unwrap();
    for (scope, count) in [
        (FileScope::Catalog, 1),
        (FileScope::Associated, 1),
        (FileScope::PageAssociated(0), 2),
        (FileScope::ObjectAssociated(id(10)), 1),
    ] {
        let files = document.files(scope).unwrap();
        assert_eq!(files.len(), count);
        for index in 0..count {
            assert_eq!(
                document
                    .file_payload(scope, index, FilePayload::Embedded(b"F"))
                    .unwrap()
                    .data()
                    .unwrap()
                    .as_ref(),
                b"primary\0payload\xff\n"
            );
            for (pair, expected) in [b"first sidecar".as_slice(), b"second sidecar\0"]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    document
                        .file_payload(
                            scope,
                            index,
                            FilePayload::Related {
                                key: b"F",
                                index: pair
                            }
                        )
                        .unwrap()
                        .data()
                        .unwrap()
                        .as_ref(),
                    expected
                );
            }
        }
        for payload in [
            FilePayload::Embedded(b"UF"),
            FilePayload::Related {
                key: b"F",
                index: 2,
            },
        ] {
            assert_eq!(
                document.file_payload(scope, 0, payload).unwrap_err().kind,
                ErrorKind::InvalidArgument
            );
        }
    }
    assert!(
        document
            .files(FileScope::PageAnnotations(0))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        document
            .file_payload(
                FileScope::PageAnnotations(0),
                0,
                FilePayload::Embedded(b"F")
            )
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert!(
        document
            .files(FileScope::ObjectAssociated(ObjectId::new(10, 1)))
            .is_err()
    );
}

#[test]
fn collection_schema_sort_values_and_encryption_declarations_remain_inert() {
    let document = open("complete").unwrap();
    let collection = document.collection().unwrap().unwrap();
    assert_eq!(collection.object, Some(id(14)));
    assert_eq!(
        open("direct-collection")
            .unwrap()
            .collection()
            .unwrap()
            .unwrap()
            .object,
        None
    );
    assert_eq!(collection.view, CollectionView::Hidden);
    assert_eq!(collection.initial_file, Some(b"primary".as_slice()));
    assert_eq!(collection.fields.len(), 4);
    let field = collection
        .fields
        .iter()
        .find(|field| field.key == b"amount")
        .unwrap();
    assert_eq!(field.kind, CollectionFieldKind::Number);
    assert_eq!(field.name, "Amount");
    assert!(!field.visible);
    assert!(!field.editable);
    assert_eq!(
        collection
            .sort
            .iter()
            .map(|sort| (sort.key.as_bytes(), sort.ascending))
            .collect::<Vec<_>>(),
        vec![
            (b"amount".as_slice(), false),
            (b"label", true),
            (b"date", true)
        ]
    );
    let files = document.files(FileScope::Catalog).unwrap();
    let file = &files[0].specification;
    assert_eq!(
        file.collection_item
            .iter()
            .find(|item| item.key == b"amount")
            .unwrap()
            .value,
        Some(CollectionValue::Integer(9_007_199_254_740_993))
    );
    let label = file
        .collection_item
        .iter()
        .find(|item| item.key == b"label")
        .unwrap();
    assert_eq!(label.prefix.as_deref(), Some("Prefix: "));
    assert_eq!(label.value, Some(CollectionValue::Text("Value".into())));
    assert_eq!(
        file.encrypted_payload.unwrap().subtype.as_bytes(),
        b"ExampleCrypt"
    );
    assert!(
        open("empty-collection")
            .unwrap()
            .collection()
            .unwrap()
            .unwrap()
            .fields
            .is_empty()
    );
    assert!(
        open("null-collection")
            .unwrap()
            .collection()
            .unwrap()
            .is_none()
    );
    assert!(
        open("nonstring-initial")
            .unwrap()
            .collection()
            .unwrap()
            .unwrap()
            .initial_file
            .is_none()
    );
    assert_eq!(
        open("navigator-inert")
            .unwrap()
            .collection()
            .unwrap()
            .unwrap()
            .view,
        CollectionView::Navigator
    );
    for name in [
        "invalid-view",
        "navigator-missing",
        "invalid-schema",
        "invalid-sort",
        "cyclic-collection",
    ] {
        assert!(open(name).unwrap().collection().is_err(), "{name}");
    }
    for name in ["invalid-item", "invalid-encrypted-payload"] {
        assert!(
            open(name).unwrap().files(FileScope::Catalog).is_err(),
            "{name}"
        );
    }
}

#[test]
fn metadata_and_source_images_do_not_conflate_source_and_rendered_appearance() {
    let document = open("complete").unwrap();
    for owner in [MetadataOwner::Page(0), MetadataOwner::Object(id(10))] {
        let packet = document.metadata_for(owner).unwrap().unwrap();
        assert_eq!(packet.object(), id(8));
        assert_eq!(
            packet.data().unwrap().as_ref(),
            b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">object packet</x:xmpmeta>"
        );
    }
    assert_eq!(
        document
            .metadata_for(MetadataOwner::Document)
            .unwrap()
            .unwrap()
            .data()
            .unwrap()
            .as_ref(),
        b"<catalog/>"
    );
    let chained = open("chained-assets").unwrap();
    assert_eq!(chained.thumbnail(id(3)).unwrap().unwrap().object(), id(12));
    assert!(
        chained
            .page_image_resources(0)
            .unwrap()
            .iter()
            .all(|image| image.asset.object() == id(9))
    );
    let images = document.page_image_resources(0).unwrap();
    assert_eq!(images.len(), 2);
    assert!(
        images
            .iter()
            .all(|resource| resource.asset.object() == id(9))
    );
    let image = document.image_asset(id(9)).unwrap();
    assert_eq!((image.width(), image.height()), (2, 1));
    assert!(!image.is_thumbnail());
    assert_eq!(
        image
            .transfer(ImageTransferMode::Encoded)
            .unwrap()
            .data
            .as_ref(),
        [255, 0, 0, 0, 255, 0]
    );
    let stored = image.transfer(ImageTransferMode::Stored).unwrap();
    assert_eq!(
        stored.data.as_ref(),
        document
            .raw_stream_data(document.object(id(9)).unwrap())
            .unwrap()
    );
    assert_ne!(stored.data.as_ref(), [255, 0, 0, 0, 255, 0]);
    for owner in [id(3), id(10)] {
        let thumb = document.thumbnail(owner).unwrap().unwrap();
        assert!(thumb.is_thumbnail());
        assert_eq!(
            thumb
                .transfer(ImageTransferMode::Encoded)
                .unwrap()
                .data
                .as_ref(),
            [127]
        );
    }
    for name in ["invalid-image-width", "invalid-image-type"] {
        assert!(open(name).unwrap().image_asset(id(9)).is_err());
    }
    for name in ["invalid-thumbnail", "invalid-thumbnail-subtype"] {
        assert!(open(name).unwrap().thumbnail(id(3)).is_err());
    }
    assert!(
        open("empty-owner")
            .unwrap()
            .metadata_for(MetadataOwner::Object(id(10)))
            .unwrap()
            .is_none()
    );
    assert!(
        open("null-owner")
            .unwrap()
            .metadata_for(MetadataOwner::Object(id(10)))
            .is_err()
    );
}

#[test]
fn authentication_creates_a_fresh_snapshot_and_failure_preserves_the_source() {
    let source = fixture("files", "encrypted-no-type.pdf");
    let document = Document::open(
        source.clone(),
        &OpenOptions {
            recovery: Recovery::Repair,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let selected = || {
        document
            .file_payload(FileScope::Catalog, 0, FilePayload::Embedded(b"F"))
            .unwrap()
    };
    assert_eq!(selected().data().unwrap_err().kind, ErrorKind::Password);
    assert_eq!(
        document.reauthenticate(b"incorrect").unwrap_err().kind,
        ErrorKind::Password
    );
    let authenticated = document.reauthenticate(b"attachment").unwrap();
    assert_eq!(
        authenticated
            .file_payload(FileScope::Catalog, 0, FilePayload::Embedded(b"F"))
            .unwrap()
            .data()
            .unwrap()
            .as_ref(),
        b"attached file bytes"
    );
    assert_eq!(selected().data().unwrap_err().kind, ErrorKind::Password);
    assert_eq!(authenticated.original_bytes(), source.as_slice());
}

#[test]
fn discovery_and_decode_limits_are_independent() {
    let source = fixture("document_services", "complete.pdf");
    let document = Document::open(
        source.clone(),
        &OpenOptions {
            limits: Limits {
                decoded_bytes: 2,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(document.page_image_resources(0).unwrap().len(), 2);
    assert_eq!(
        document
            .image_asset(id(9))
            .unwrap()
            .transfer(ImageTransferMode::Encoded)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let document = Document::open(
        source,
        &OpenOptions {
            limits: Limits {
                traversal_work: 1,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(document.collection().unwrap_err().kind, ErrorKind::Limit);
}
