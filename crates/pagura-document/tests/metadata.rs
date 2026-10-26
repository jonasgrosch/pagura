#![forbid(unsafe_code)]
//! Public metadata API against independently authored source bytes.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery};
use pagura_document::{Document, OpenOptions, metadata::Trapped};
use pagura_testkit::fixture;

fn open(name: &str, options: &OpenOptions) -> Document {
    Document::open(fixture("metadata", &format!("{name}.pdf")), options).unwrap()
}

#[test]
fn information_and_xmp_are_distinct_immutable_sources() {
    let packet = b"<?xpacket begin=\"\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><title>XMP title</title></x:xmpmeta><?xpacket end=\"w\"?>";
    for name in ["populated", "populated-flate"] {
        let document = open(name, &OpenOptions::default());
        let info = document.information().unwrap().unwrap();
        assert_eq!(info.object, Some(ObjectId::new(5, 0)));
        assert_eq!(info.title.as_deref(), Some("Grüße Ω"));
        assert_eq!(info.author.as_deref(), Some("Euro €"));
        assert_eq!(info.subject.as_deref(), Some("測試"));
        assert_eq!(info.keywords.as_deref(), Some("one; two"));
        assert_eq!(info.creator.as_deref(), Some("Original app"));
        assert_eq!(info.producer.as_deref(), Some("PDF app"));
        assert_eq!(
            info.creation_date.as_deref(),
            Some("D:20261026030000+01'00")
        );
        assert_eq!(info.modification_date.as_deref(), Some("D:20261026040000Z"));
        assert_eq!(info.trapped, Trapped::True);
        assert!(info.dictionary.contains_key(b"Custom"));
        assert_eq!(document.language().unwrap().as_deref(), Some("de-DE"));
        let metadata = document.metadata().unwrap().unwrap();
        assert_eq!(metadata.object(), ObjectId::new(6, 0));
        assert!(metadata.source().is_stream());
        assert_eq!(metadata.data().unwrap().as_ref(), packet);
        let page = document.object(ObjectId::new(3, 0)).unwrap();
        let local = document
            .object_metadata(page.dictionary().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(local.object(), ObjectId::new(7, 0));
        assert_eq!(
            local.data().unwrap().as_ref(),
            b"<object-metadata>separate packet</object-metadata>"
        );
        assert_eq!(
            document.original_bytes(),
            fixture("metadata", &format!("{name}.pdf"))
        );
    }
}

#[test]
fn absence_null_empty_and_unknown_have_distinct_meanings() {
    for name in ["absent", "null", "undefined"] {
        let document = open(name, &OpenOptions::default());
        assert!(document.information().unwrap().is_none());
        assert!(document.metadata().unwrap().is_none());
        assert!(document.language().unwrap().is_none());
    }
    let document = open("empty", &OpenOptions::default());
    let info = document.information().unwrap().unwrap();
    assert!(info.title.is_none());
    assert_eq!(info.trapped, Trapped::Unknown);
    assert!(
        document
            .metadata()
            .unwrap()
            .unwrap()
            .data()
            .unwrap()
            .is_empty()
    );
    for (name, expected) in [
        ("trapped-false", Trapped::False),
        ("trapped-unknown", Trapped::Unknown),
    ] {
        assert_eq!(
            open(name, &OpenOptions::default())
                .information()
                .unwrap()
                .unwrap()
                .trapped,
            expected
        );
    }
}

#[test]
fn malformed_types_and_cycles_fail_at_the_lazy_view_boundary() {
    for name in [
        "info-wrong-type",
        "info-field-wrong-type",
        "info-trapped-boolean",
    ] {
        assert_eq!(
            open(name, &OpenOptions::default())
                .information()
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    let field_error = open("info-field-wrong-type", &OpenOptions::default())
        .information()
        .unwrap_err();
    assert_eq!(field_error.object, Some(ObjectId::new(8, 0)));
    assert_eq!(
        open("info-cycle", &OpenOptions::default())
            .information()
            .unwrap_err()
            .kind,
        ErrorKind::Cycle
    );
    assert_eq!(
        open("language-wrong-type", &OpenOptions::default())
            .language()
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    for name in [
        "metadata-direct-dictionary",
        "metadata-nonstream",
        "metadata-missing-type",
        "metadata-wrong-subtype",
    ] {
        assert_eq!(
            open(name, &OpenOptions::default())
                .metadata()
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    assert_eq!(
        open("metadata-cycle", &OpenOptions::default())
            .metadata()
            .unwrap_err()
            .kind,
        ErrorKind::Cycle
    );
}

#[test]
fn text_recovery_and_resource_limits_are_not_silent_successes() {
    let strict = OpenOptions {
        recovery: Recovery::Strict,
        ..OpenOptions::default()
    };
    let error = open("invalid-text", &strict).information().unwrap_err();
    assert_eq!(error.repair, Some(pagura_core::repairs::INVALID_TEXT));
    let document = open("invalid-text", &OpenOptions::default());
    assert_eq!(
        document.information().unwrap().unwrap().title.as_deref(),
        Some("�")
    );
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|d| d.repair() == error.repair)
    );
    for limits in [
        Limits {
            string_bytes: 9,
            ..Limits::default()
        },
        Limits {
            traversal_work: 0,
            ..Limits::default()
        },
    ] {
        let document = open(
            "info-string",
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        );
        assert_eq!(document.information().unwrap_err().kind, ErrorKind::Limit);
    }
    let document = open(
        "populated-flate",
        &OpenOptions {
            limits: Limits {
                decoded_bytes: 16,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    );
    assert_eq!(
        document
            .metadata()
            .unwrap()
            .unwrap()
            .data()
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn xml_is_returned_as_inert_bytes_without_entity_expansion() {
    let document = open("metadata-inert-xml", &OpenOptions::default());
    assert_eq!(
        document
            .metadata()
            .unwrap()
            .unwrap()
            .data()
            .unwrap()
            .as_ref(),
        b"<!DOCTYPE x [<!ENTITY e SYSTEM \"https://example.invalid/metadata\">]><x>&e;</x>"
    );
}
