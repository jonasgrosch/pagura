//! Selected checked-CID and RequiredNamed policies retain separate boundaries.
use crate::support::ppm;
use pagura::{
    Document, Error, ErrorKind, OpenOptions, Recovery, RenderContext, RenderOptions, Result,
};
#[cfg(feature = "bundled-data")]
use pagura::{ObjectId, Position};
#[cfg(feature = "bundled-data")]
use pagura_testkit::Manifest;
use pagura_testkit::fixture;
use std::sync::Arc;

const AREA: &str = "selected_font_interactions";

fn open(name: &str, recovery: Recovery) -> Result<RenderContext> {
    let document = Document::open(
        fixture(AREA, &format!("{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )?;
    Ok(pagura::render_context(Arc::new(document)))
}

#[test]
fn descriptor_alias_does_not_inherit_native_first_collection_face() -> Result<()> {
    for name in [
        "alias-ttc-single",
        "alias-ttc-pair-first",
        "alias-ttc-pair-second",
        "alias-ttc-ambiguous",
    ] {
        let mut errors = Vec::new();
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let context = open(name, recovery)?;
            let page = context.document().page(0)?;
            let result = context.render_page(page, &RenderOptions::default());
            let Err(error) = result else {
                return Err(Error::syntax(
                    "alias collection unexpectedly selected a face",
                ));
            };
            assert_eq!(
                error.kind,
                ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED),
                "{name}"
            );
            assert_eq!(
                error.feature().map(pagura::Feature::as_str),
                Some("font.non-embedded")
            );
            assert_eq!(error.repair, None);
            errors.push(error);
        }
        assert_eq!(errors[0], errors[1], "{name}");
    }
    let context = open("canonical-ttc-second", Recovery::Repair)?;
    let rendered = context.render_page(context.document().page(0)?, &RenderOptions::default())?;
    assert_eq!(ppm(&rendered), fixture(AREA, "canonical-ttc-second.ppm"));
    Ok(())
}

#[test]
#[cfg(feature = "bundled-data")]
fn mixed_required_policies_keep_pixels_and_full_refusals() -> Result<()> {
    let manifest = Manifest::load(AREA);
    assert_eq!(manifest.cases().len(), 13);
    for case in manifest.cases().filter(|case| !case.name().contains("ttc")) {
        let context = open(case.name(), Recovery::Repair)?;
        let page_index = usize::from(case.name().starts_with("later-"));
        if page_index != 0 {
            let prior =
                context.render_page(context.document().page(0)?, &RenderOptions::default())?;
            let bytes = ppm(&prior);
            assert_eq!(bytes.len(), b"P6\n64 64\n255\n".len() + 64 * 64 * 3);
            assert!(
                bytes[b"P6\n64 64\n255\n".len()..]
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .all(|rgb| *rgb == [0, 0, 255])
            );
            drop(prior);
        }
        let page = context.document().page(page_index)?;
        let result = context.render_page(page, &RenderOptions::default());
        if case.string("expected") == Some("render") {
            let actual = ppm(&result?);
            let expected = fixture(AREA, &format!("{}.ppm", case.name()));
            let header = b"P6\n64 64\n255\n";
            assert_eq!(actual.len(), expected.len(), "{}", case.id());
            assert!(actual.starts_with(header) && expected.starts_with(header));
            assert!(
                actual[header.len()..]
                    .iter()
                    .zip(&expected[header.len()..])
                    .all(|(a, b)| a.abs_diff(*b) <= 1),
                "{}",
                case.id()
            );
        } else {
            let Err(error) = result else {
                return Err(Error::syntax(
                    "missing required glyph unexpectedly rendered",
                ));
            };
            assert!(matches!(error.kind, ErrorKind::Unsupported(_)));
            assert_eq!(
                error.feature().map(pagura::Feature::as_str),
                case.string("feature")
            );
            let retained = context.retained_bytes();
            let again = context.render_page(page, &RenderOptions::default());
            assert!(matches!(again, Err(ref second) if second == &error));
            assert_eq!(context.retained_bytes(), retained);
        }
    }
    Ok(())
}

#[test]
#[cfg(feature = "bundled-data")]
fn mixed_source_widths_and_bound_descriptor_determine_text_geometry() -> Result<()> {
    let context = open("source-widths-bbox", Recovery::Repair)?;
    let text = context.text_page(context.document().page(0)?)?;
    let glyphs = text.glyphs();
    assert_eq!(glyphs.len(), 3);
    assert_eq!(glyphs[0].unicode.as_deref(), Some("H"));
    assert_eq!(glyphs[0].advance_end.x - glyphs[0].origin.x, 24.0);
    assert_ne!(glyphs[0].font, glyphs[1].font);
    assert_eq!(glyphs[1].font, glyphs[2].font);
    // Literal frozen source-widths-bbox witness: source Widths500 and
    // GontDescriptor y[-200,800], independently of the supplied headBBox.
    for (glyph, x) in glyphs[1..].iter().zip([40.0, 50.0]) {
        assert_eq!(glyph.unicode.as_deref(), Some("A"));
        assert_eq!(glyph.advance_end.x - glyph.origin.x, 10.0);
        let expected = [(x, 0.0), (x + 10.0, 0.0), (x + 10.0, 20.0), (x, 20.0)];
        for (point, expected) in glyph.quad.0.iter().zip(expected) {
            assert_eq!((point.x, point.y), expected);
        }
    }
    Ok(())
}

#[test]
#[cfg(feature = "bundled-data")]
fn mixed_strict_first_faults_keep_complete_source_errors() -> Result<()> {
    let names = [
        "mixed-visible",
        "empty-nonzero-alias-space",
        "source-widths-bbox",
        "after-visible-alias-missing",
        "invisible-alias-missing",
        "later-first-alias-missing",
        "later-first-alias-valid",
        "cid-hole-with-empty-alias",
    ];
    for name in names {
        let context = open(name, Recovery::Strict)?;
        let page = context
            .document()
            .page(usize::from(name.starts_with("later-")))?;
        let (message, offset, object) = if name == "cid-hole-with-empty-alias" {
            // Original Encoding has no0303 CID. The independent raw content
            // places its Tj at34; the interpreter attributes decoding to page3.
            (
                "checked CID substitute requires a non-notdef Encoding mapping for every used code",
                34,
                3,
            )
        } else {
            // The original source reaches the alias font's Tf at49, font5.
            ("font has no safe available substitute", 49, 5)
        };
        let expected = Error::unsupported(pagura_text::features::NON_EMBEDDED, message)
            .at(Position::Stream {
                object: ObjectId::new(4, 0),
                offset,
            })
            .in_object(ObjectId::new(object, 0));
        for _ in 0..2 {
            let Err(error) = context.render_page(page, &RenderOptions::default()) else {
                return Err(Error::syntax(
                    "Strict mixed interaction unexpectedly rendered",
                ));
            };
            assert_eq!(error, expected, "{name}");
        }
        assert!(
            matches!(context.text_page(page), Err(error) if error == expected),
            "{name}"
        );
    }
    Ok(())
}
