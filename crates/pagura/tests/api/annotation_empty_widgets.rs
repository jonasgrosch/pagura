//! Empty source widgets select geometry without requesting unused field text.
use crate::support::ppm;
use pagura::{Document, ErrorKind, ObjectId, OpenOptions, Recovery, RenderOptions};
use pagura_testkit::{Manifest, fixture};

#[test]
fn empty_widget_geometry_and_stored_artwork_keep_their_lazy_boundaries() {
    let manifest = Manifest::load("annotation_empty_widgets");
    for case in manifest.cases() {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let name = case.name();
            let document = Document::open(
                fixture("annotation_empty_widgets", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )
            .unwrap();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::from_dpi(72.0),
            );
            match case.string("expected").unwrap() {
                "render" => assert_eq!(
                    ppm(&result.unwrap()),
                    fixture("annotation_empty_widgets", &format!("{name}.ppm")),
                    "{name}"
                ),
                "unsupported" => assert_eq!(
                    result.unwrap_err().feature().map(pagura::Feature::as_str),
                    case.string("feature"),
                    "{name}"
                ),
                _ => assert_eq!(
                    result.unwrap_err().kind,
                    if case.string("render_error") == Some("cycle") {
                        ErrorKind::Cycle
                    } else {
                        ErrorKind::Syntax
                    },
                    "{name}"
                ),
            }
        }
    }
}

#[test]
fn complete_field_inspection_and_form_editing_keep_their_existing_validation() {
    use pagura::forms::FillValue;
    for name in [
        "wrong-da",
        "text-v-utf16be-wrong-da",
        "text-v-utf8-wrong-da",
        "text-rv-utf16be-wrong-da",
        "text-rv-utf8-wrong-da",
    ] {
        let document = Document::open(
            fixture("annotation_empty_widgets", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let widget = document
            .object(ObjectId::new(11, 0))
            .unwrap()
            .dictionary()
            .unwrap();
        assert!(document.widget_field_membership(widget).unwrap());
        assert_eq!(
            document.widget_field(widget).unwrap_err().kind,
            ErrorKind::Syntax,
            "{name}"
        );
    }
    let document = Document::open(
        fixture("annotation_empty_widgets", "no-scroll.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    assert_eq!(
        pagura::form_appearance::prepare(&filler, None)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    let document = Document::open(
        fixture("annotation_empty_widgets", "xfa.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    assert_eq!(
        filler
            .set(0, FillValue::Text("AB"))
            .unwrap_err()
            .feature()
            .map(pagura::Feature::as_str),
        Some("form.xfa")
    );
}

#[test]
fn empty_membership_proof_obeys_the_existing_navigation_work_limit() {
    let document = Document::open(
        fixture("annotation_empty_widgets", "missing-font.pdf"),
        &OpenOptions {
            limits: pagura::Limits {
                traversal_work: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        document
            .widget_field_membership(
                document
                    .object(ObjectId::new(11, 0))
                    .unwrap()
                    .dictionary()
                    .unwrap()
            )
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn legacy_empty_ancestry_comparisons_are_bounded_by_traversal_work() {
    for work in [100, 2000] {
        let document = Document::open(
            fixture("annotation_empty_widgets", "legacy-long-ancestry.pdf"),
            &OpenOptions {
                limits: pagura::Limits {
                    depth: 256,
                    traversal_work: work,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let result = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::from_dpi(72.0),
        );
        if work == 100 {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert_eq!(error.message, "widget ancestry work budget exceeded");
        } else {
            assert_eq!(
                ppm(&result.unwrap()),
                fixture("annotation_empty_widgets", "legacy-long-ancestry.ppm")
            );
        }
    }
}
