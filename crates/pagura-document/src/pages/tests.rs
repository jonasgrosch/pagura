use crate::{Document, OpenOptions};
use pagura_testkit::fixture;
use std::sync::Arc;

#[test]
fn pages_share_one_copy_of_each_resource_dictionary() {
    let document = Document::open(
        fixture("document", "pages-shared-resources.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let pages = document.pages().unwrap();
    let [inherited, sibling, direct, own] = pages else {
        panic!("expected four pages, found {}", pages.len());
    };
    // Pages 3, 4 and 9 use object 7 (two inherit it, one names it itself).
    assert!(Arc::ptr_eq(&inherited.resources, &sibling.resources));
    assert!(Arc::ptr_eq(&inherited.resources, &own.resources));
    assert!(inherited.resources().contains_key(b"ExtGState"));
    // Page 8 inherits node 5's direct dictionary instead.
    assert!(!Arc::ptr_eq(&inherited.resources, &direct.resources));
    assert!(!direct.resources().contains_key(b"ExtGState"));
    assert_eq!(direct.media_box(), inherited.media_box());
    // The page dictionary is the page object's own, without inherited entries.
    assert!(!inherited.dictionary().contains_key(b"Resources"));
    assert!(!inherited.dictionary().contains_key(b"MediaBox"));
}

mod retained_tests {
    use crate::{Document, Page};
    use pagura_core::{Dictionary, ErrorKind, Limits};

    use std::sync::Arc;

    fn open(bytes: usize) -> Document {
        let document = crate::tests::document(
            "pages-shared-resources",
            pagura_core::Recovery::default(),
            Limits {
                document_bytes: bytes,
                ..Limits::default()
            },
        );
        for id in document.object_ids() {
            document.object(id).unwrap();
        }
        document
    }

    #[test]
    fn shared_resource_storage_is_charged_once_and_failed_walks_release_reservations() {
        let document = open(usize::MAX);
        let initial = document.retained.used();
        let pages = document.pages().unwrap();
        let mut unique = Vec::new();
        let mut expected = 0;
        for page in pages {
            expected +=
                size_of::<Page>() + page.dictionary.retained_bytes() - size_of::<Dictionary>();
            if !unique
                .iter()
                .any(|resource| Arc::ptr_eq(resource, &page.resources))
            {
                unique.push(Arc::clone(&page.resources));
                expected += page.resources.retained_bytes() + 2 * size_of::<usize>();
            }
        }
        assert_eq!(unique.len(), 2);
        assert_eq!(document.retained.used(), initial + expected);
        let exact = open(initial + expected);
        assert_eq!(exact.page_count().unwrap(), 4);
        assert_eq!(exact.retained.used(), initial + expected);
        exact.pages().unwrap();
        assert_eq!(exact.retained.used(), initial + expected);
        let limited = open(initial + expected - 1);
        let error = limited.pages().unwrap_err();
        assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
        assert!(error.as_error().unwrap().object.is_some());
        assert_eq!(limited.retained.used(), initial);
        assert_eq!(limited.pages().unwrap_err(), error);
        assert_eq!(limited.retained.used(), initial);
    }
}

mod recovery_tests {
    use crate::{Document, OpenOptions, Page, repairs};
    use pagura_core::{Dictionary, ErrorKind, Limits, ObjectId, Recovery, Rect};
    use pagura_testkit::fixture;
    use std::sync::Arc;

    #[test]
    fn page_repairs_reject_strictly_and_preserve_source_and_geometry() {
        for (name, tag, count) in [
            ("pages-media-missing", repairs::PAGE_MEDIA_BOX, 1),
            ("pages-media-empty", repairs::PAGE_MEDIA_BOX, 1),
            ("pages-media-from-crop", repairs::PAGE_MEDIA_BOX, 1),
            ("pages-crop-empty", repairs::PAGE_CROP_BOX, 1),
            ("pages-crop-disjoint", repairs::PAGE_CROP_BOX, 1),
            ("pages-kids-invalid", repairs::PAGE_KIDS, 1),
            ("pages-kids-single", repairs::PAGE_KIDS, 1),
            ("pages-kids-rebuilt", repairs::PAGE_KIDS_REBUILT, 1),
            ("pages-root-untyped-backlinks", repairs::PAGE_TYPE, 1),
            ("pages-repeat-leaf", repairs::PAGE_REPEAT, 2),
            ("pages-repeat-branch", repairs::PAGE_REPEAT, 2),
            ("pages-cycle-usable", repairs::PAGE_CYCLE, 1),
            ("pages-parent-mismatch", repairs::PAGE_PARENT, 1),
            ("pages-template-kid", repairs::PAGE_TYPE_INVALID, 1),
            ("pages-direct-kids", repairs::PAGE_DIRECT, 2),
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                strict.pages().unwrap_err().as_error().unwrap().repair,
                Some(tag),
                "{name}"
            );
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            let pages = document.pages().unwrap();
            assert_eq!(pages.len(), count, "{name}");
            assert_eq!(document.materialize().unwrap().bytes(), bytes);
            let letter = matches!(name, "pages-media-missing" | "pages-media-empty");
            let size = if letter { [612.0, 792.0] } else { [64.0, 64.0] };
            let expected = Rect::from_corners(0.0, 0.0, size[0], size[1]).unwrap();
            assert_eq!(pages[0].media_box(), expected, "{name}");
            assert_eq!(pages[0].crop_box(), expected, "{name}");
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .any(|d| d.repair() == Some(tag))
            );
        }
    }

    fn preloaded(name: &str, bytes: usize) -> Document {
        let document = Document::open(
            fixture("document", &format!("{name}.pdf")),
            &OpenOptions {
                limits: Limits {
                    document_bytes: bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        for id in document.object_ids() {
            document.object(id).unwrap();
        }
        document
    }

    #[test]
    fn recovered_pages_use_one_cache_and_transactional_memory_reservations() {
        for name in [
            "pages-repeat-leaf",
            "pages-repeat-branch",
            "pages-cycle-usable",
            "pages-direct-kids",
            "pages-root-untyped-backlinks",
            "pages-media-missing",
            "xref-swapped-offsets",
            "object-stream-type-invalid",
            "object-stream-self-entry",
            "direct-stream-array",
            "direct-stream-indirect-array",
            "direct-stream-direct-page",
            "direct-stream-indirect-kids",
            "direct-stream-resources",
            "direct-stream-encrypted",
            "security-length-missing",
            "security-zero-padded-40",
            "security-zero-padded-48",
            "security-legacy-permission-selector",
            "catalog-number-zero",
        ] {
            let document = preloaded(name, 128 * 1024 * 1024);
            let initial = document.retained.used();
            let pages = document.pages().unwrap();
            let count = pages.len();
            let retained = document.retained.used();
            let mut unique = Vec::new();
            let mut expected = 0;
            for page in pages {
                expected += size_of::<Page>() + page.dictionary.retained_bytes()
                    - size_of::<Dictionary>()
                    + page
                        .source_path
                        .iter()
                        .map(pagura_core::syntax::PathComponent::retained_bytes)
                        .sum::<usize>();
                if !unique
                    .iter()
                    .any(|resource| Arc::ptr_eq(resource, &page.resources))
                {
                    unique.push(Arc::clone(&page.resources));
                    expected += page.resources.retained_bytes() + 2 * size_of::<usize>();
                }
            }
            assert_eq!(retained, initial + expected, "{name}");
            assert!(retained > initial, "{name}");
            document.pages().unwrap();
            assert_eq!(document.retained.used(), retained, "{name}");
            let exact = preloaded(name, retained);
            assert_eq!(exact.page_count().unwrap(), count, "{name}");
            assert_eq!(exact.retained.used(), retained, "{name}");
            let limited = preloaded(name, retained - 1);
            let error = limited.pages().unwrap_err();
            assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit, "{name}");
            assert!(error.as_error().unwrap().object.is_some());
            assert_eq!(limited.retained.used(), initial, "{name}");
            assert_eq!(limited.pages().unwrap_err(), error);
        }
        let repeated = preloaded("pages-repeat-leaf", usize::MAX);
        let pages = repeated.pages().unwrap();
        assert_eq!(pages[0].id(), ObjectId::new(3, 0));
        assert_eq!(pages[1].id(), pages[0].id());
        assert_eq!(pages[1].index(), 1);
        assert!(Arc::ptr_eq(&pages[0].resources, &pages[1].resources));
        let direct = preloaded("pages-direct-kids", usize::MAX);
        let pages = direct.pages().unwrap();
        assert_eq!(pages[0].id(), ObjectId::new(2, 0));
        assert!(!Arc::ptr_eq(&pages[0].resources, &pages[1].resources));
        assert!(pages[0].resources().contains_key(b"ProcSet"));
        assert!(pages[1].resources().contains_key(b"ExtGState"));
    }
    #[test]
    fn expanded_page_graphs_obey_work_and_page_limits_without_partial_results() {
        for work in [true, false] {
            let mut document = preloaded("pages-repeat-branch", usize::MAX);
            let initial = document.retained.used();
            if work {
                document.limits.traversal_work = 4;
            } else {
                document.limits.pages = 1;
            }
            let error = document.pages().unwrap_err();
            assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
            assert!(error.as_error().unwrap().object.is_some());
            assert_eq!(document.retained.used(), initial);
            assert_eq!(document.pages().unwrap_err(), error);
        }
        let strict = Document::open(
            fixture("document", "xref-swapped-offsets.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(strict.repair, Some(repairs::XREF_REBUILT));
        for (name, tag) in [
            ("object-stream-type-invalid", repairs::OBJECT_STREAM_TYPE),
            (
                "object-stream-self-entry",
                repairs::OBJECT_STREAM_SELF_ENTRY,
            ),
        ] {
            let error = Document::open(
                fixture("document", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.repair, Some(tag));
        }
    }
}
