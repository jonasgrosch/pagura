//! Annotation source display and explicit missing-geometry recovery.

use crate::support::ppm;
use pagura::{Document, ErrorKind, OpenOptions, Recovery, RenderOptions};
use pagura_testkit::{Json, Manifest, assert_bytes_eq, fixture};

#[test]
fn annotation_rectangles_and_inert_popup_repairs_preserve_page_paint() {
    let manifest = Manifest::load("annotation_compat");
    for case in manifest
        .cases()
        .filter(|case| case.string("expected") == Some("render"))
    {
        let name = case.name();
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("annotation_compat", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let dpi = case.get("dpi").and_then(Json::as_f64).unwrap();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::from_dpi(dpi),
            );
            if recovery == Recovery::Strict
                && case.get("strict_valid").and_then(Json::as_bool) == Some(false)
            {
                assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            } else {
                assert_bytes_eq(
                    &ppm(&result.unwrap()),
                    &fixture("annotation_compat", &format!("{name}.ppm")),
                    name,
                );
                let diagnostics = document.diagnostics();
                if name.starts_with("popup-") {
                    assert_eq!(
                        diagnostics
                            .iter()
                            .map(pagura::Diagnostic::tag)
                            .collect::<Vec<_>>(),
                        ["annotation.popup-missing-rect"],
                        "{name}"
                    );
                } else {
                    assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
                }
            }
        }
    }
}

#[test]
fn annotation_geometry_errors_never_turn_supplied_artwork_into_success() {
    for name in [
        "popup-stored-missing-rect",
        "popup-short-rect",
        "popup-wrong-rect",
        "square-missing-rect",
        "popup-rect-cycle",
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("annotation_compat", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let result = pagura::record_page(&document, document.page(0).unwrap());
            let error = result.unwrap_err();
            assert_eq!(
                error.kind,
                if name == "popup-rect-cycle" {
                    ErrorKind::Limit
                } else {
                    ErrorKind::Syntax
                },
                "{name}: {recovery:?}"
            );
            assert!(
                !document
                    .diagnostics()
                    .iter()
                    .any(|d| d.tag() == "annotation.popup-missing-rect"),
                "{name}"
            );
        }
    }
}

#[test]
fn annotation_display_matches_authored_geometry_and_text_oracles() {
    let mut failures = Vec::new();
    for area in [
        "rich_annotations",
        "relief_annotations",
        "stamp_annotations",
        "markup_annotations",
        "ending_annotations",
        "annotation_display",
        "caption_annotations",
        "rounded_annotations",
        "leader_annotations",
        "line_annotations",
        "ink_annotations",
        "polygon_annotations",
        "file_attachment_icons",
        "form_borders",
        "caret_annotations",
        "cloud_annotations",
        "free_text_annotations",
        "circle_annotations",
    ] {
        let manifest = Manifest::load(area);
        for case in manifest
            .cases()
            .filter(|case| case.string("expected") == Some("render"))
        {
            let name = case.name();
            let document = Document::open(
                fixture(area, &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            let dpi = case.get("dpi").and_then(Json::as_f64).unwrap();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::from_dpi(dpi),
            );
            if !pagura::provider::BUNDLED
                && case
                    .get("information")
                    .and_then(Json::as_array)
                    .is_some_and(|tags| {
                        tags.iter()
                            .any(|tag| tag.as_str() == Some("font.substituted"))
                    })
            {
                let error = result.unwrap_err();
                assert_eq!(
                    error.feature().map(pagura::Feature::as_str),
                    Some("font.non-embedded"),
                    "{area}/{name}: {error}"
                );
                continue;
            }
            let actual = result.unwrap_or_else(|e| panic!("{area}/{name}: {e}"));
            assert_eq!(
                u64::from(actual.width()),
                case.get("width").and_then(Json::as_u64).unwrap(),
                "{area}/{name}"
            );
            assert_eq!(
                u64::from(actual.height()),
                case.get("height").and_then(Json::as_u64).unwrap(),
                "{area}/{name}"
            );
            let actual = ppm(&actual);
            let expected = fixture(area, &format!("{name}.ppm"));
            let tolerance = case.get("tolerance").and_then(Json::as_u64).unwrap_or(0);
            assert_eq!(actual.len(), expected.len(), "{area}/{name}");
            let max = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .max()
                .unwrap_or(0);
            if max > tolerance {
                failures.push(format!("{area}/{name}: max {max} > {tolerance}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn annotation_render_budget_failure_precedes_band_consumers() {
    use std::sync::Arc;
    let document = Arc::new(
        Document::open(
            fixture("annotation_compat", "link-direct-rect-72.pdf"),
            &OpenOptions {
                limits: pagura::Limits {
                    render_work: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let context = pagura::RenderContext::new(document);
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let mut calls = 0;
    for _ in 0..2 {
        let error = context
            .render_bands(
                &scene,
                &RenderOptions::from_dpi(72.0),
                None,
                |_| false,
                |_, _| {
                    calls += 1;
                    Ok::<(), ()>(())
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
    assert_eq!(calls, 0);
}

#[test]
fn unqualified_curved_stroke_synthesis_is_an_explicit_refusal() {
    for area in [
        "rounded_annotations",
        "ink_annotations",
        "cloud_annotations",
    ] {
        let manifest = Manifest::load(area);
        for case in manifest
            .cases()
            .filter(|case| case.string("expected") == Some("unsupported"))
        {
            let name = case.name();
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture(area, &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..Default::default()
                    },
                )
                .unwrap();
                let error = pagura::record_page(&document, document.page(0).unwrap()).unwrap_err();
                assert_eq!(
                    error.feature().map(pagura::Feature::as_str),
                    case.string("feature"),
                    "{area}/{name}"
                );
            }
        }
    }
}
