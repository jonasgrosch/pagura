//! Supplied program identity, source selection/metrics and descriptor geometry.
use crate::support::ppm;
use pagura::{
    Document, Error, ErrorKind, ObjectId, OpenOptions, Position, Recovery, RenderContext,
    RenderOptions, Result,
};
use pagura_testkit::{Json, Manifest, fixture};
use pagura_text::{GlyphShape, PdfFont};
use std::sync::Arc;

const AREA: &str = "recovery_font_descriptor";

struct FailedProvider;
impl pagura::provider::Provider for FailedProvider {
    fn cid_font(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<pagura::provider::FontProgram>>> {
        Ok(None)
    }
    fn standard_font(&self, _: &[u8]) -> Result<Option<Arc<pagura::provider::StandardFont>>> {
        Err(provider_error())
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<pagura::provider::PredefinedCMap>> {
        Ok(None)
    }
    fn cid_to_unicode(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<Option<Arc<pagura::provider::CidToUnicode>>> {
        Ok(None)
    }
}

fn provider_error() -> Error {
    Error::syntax("independent provider refusal")
        .at(Position::Buffer(17))
        .in_object(ObjectId::new(77, 0))
}

fn open(name: &str, recovery: Recovery) -> Result<Document> {
    Document::open(
        fixture(AREA, &format!("{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
}

fn refused<T>(result: Result<T>) -> Result<Error> {
    match result {
        Err(error) => Ok(error),
        Ok(_) => Err(Error::syntax("expected the independently declared refusal")),
    }
}

fn dictionary(document: &Document) -> Result<&pagura::Dictionary> {
    document
        .object(ObjectId::new(5, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("fixture font dictionary missing"))
}

#[test]
fn all_frozen_supplied_program_pages_have_complete_output_or_refusal() -> Result<()> {
    let manifest = Manifest::load(AREA);
    assert_eq!(manifest.cases().len(), 64);
    for case in manifest.cases() {
        let context = RenderContext::new(Arc::new(open(case.name(), Recovery::Repair)?));
        let page_index = usize::try_from(case.get("page").and_then(Json::as_u64).unwrap_or(0))
            .map_err(|_| Error::syntax("fixture page index overflow"))?;
        if page_index != 0 {
            context.render_page(context.document().page(0)?, &RenderOptions::default())?;
        }
        let page = context.document().page(page_index)?;
        let result = context.render_page(page, &RenderOptions::default());
        if case.string("expected") == Some("render") {
            assert_eq!(
                ppm(&result?),
                fixture(AREA, &format!("{}.ppm", case.name())),
                "{}",
                case.id()
            );
        } else {
            let first = refused(result)?;
            if let Some(feature) = case.string("feature") {
                assert_eq!(
                    first.feature().map(pagura::Feature::as_str),
                    Some(feature),
                    "{}",
                    case.id()
                );
            } else {
                assert_eq!(first.kind, ErrorKind::Syntax, "{}", case.id());
            }
            let retained = context.retained_bytes();
            let again = refused(context.render_page(page, &RenderOptions::default()))?;
            assert_eq!(first, again, "{}", case.id());
            assert_eq!(context.retained_bytes(), retained, "{}", case.id());
        }
    }
    Ok(())
}

#[test]
fn required_selection_checks_used_codes_with_and_without_unicode() -> Result<()> {
    let document = open("selection-named-windows-holes", Recovery::Repair)?;
    let dictionary = dictionary(&document)?;
    let font = PdfFont::load(&document, dictionary)?;
    let unicode = font.unicode(&document, dictionary, None)?;
    for extraction in [None, Some(&unicode)] {
        assert!(font.decode(b"", extraction)?.is_empty());
        for input in [b"A".as_slice(), b"AA", b"A"] {
            let glyphs = font.decode(input, extraction)?;
            assert_eq!(glyphs.len(), input.len());
            for glyph in glyphs {
                assert_eq!(glyph.advance, 0.5);
                assert!(matches!(glyph.shape, GlyphShape::Outline { glyph, .. } if glyph.0 == 1));
            }
        }
        for input in [b"B".as_slice(), b"B", b"AB"] {
            let error = refused(font.decode(input, extraction))?;
            assert_eq!(
                error.feature().map(pagura::Feature::as_str),
                Some("font.descriptor-glyph")
            );
            assert_eq!(error.position, Position::Buffer(input.len() - 1));
        }
    }
    Ok(())
}

#[test]
fn explicit_encoding_names_cannot_be_rescued_by_byte_or_unicode_fallback() -> Result<()> {
    for name in [
        "encoding-vs-symbol-byte",
        "encoding-vs-symbol-byte-tounicode",
        "explicit-gid-zero",
        "source-encoding-hole",
        "partial-source-glyph-name",
        "glyph-name-suffix",
        "multi-scalar-name",
    ] {
        let document = open(&format!("selection-{name}"), Recovery::Repair)?;
        let dictionary = dictionary(&document)?;
        let font = PdfFont::load(&document, dictionary)?;
        let unicode = font.unicode(&document, dictionary, None)?;
        for extraction in [None, Some(&unicode)] {
            for _ in 0..2 {
                let error = refused(font.decode(b"A", extraction))?;
                assert_eq!(
                    error.feature().map(pagura::Feature::as_str),
                    Some("font.descriptor-glyph")
                );
            }
        }
    }
    Ok(())
}

#[test]
fn strict_used_glyph_validation_precedes_successful_cache_entries() -> Result<()> {
    let document = open("selection-strict-used-glyph", Recovery::Repair)?;
    let font_dictionary = dictionary(&document)?;
    let font = PdfFont::load(&document, font_dictionary)?;
    let unicode = font.unicode(&document, font_dictionary, None)?;
    for extraction in [None, Some(&unicode)] {
        let first = refused(font.decode(b"A", extraction))?;
        assert_eq!(first.kind, ErrorKind::Syntax);
        assert_eq!(
            first.repair.map(pagura::Repair::as_str),
            Some("font.glyph-data")
        );
        assert_eq!(refused(font.decode(b"A", extraction))?, first);
    }
    let document = open("selection-valid-empty-nonzero-space", Recovery::Repair)?;
    let font = PdfFont::load(&document, dictionary(&document)?)?;
    let glyphs = font.decode(b" ", None)?;
    assert_eq!(glyphs.len(), 1);
    assert_eq!(glyphs[0].advance, 0.5);
    assert!(
        matches!(&glyphs[0].shape, GlyphShape::Outline { program, glyph, .. }
                    if glyph.0 == 1 && program.outline(*glyph).is_ok_and(|outline| outline.is_empty()))
    );
    Ok(())
}

#[test]
fn strict_keeps_original_unavailable_error_without_an_alias_repair() -> Result<()> {
    let document = open("descriptor-alias", Recovery::Strict)?;
    let dictionary = dictionary(&document)?;
    let original = refused(PdfFont::load(&document, dictionary))?;
    assert_eq!(
        original.feature().map(pagura::Feature::as_str),
        Some("font.non-embedded")
    );
    assert_eq!(original.message, "font program is not embedded");
    assert_eq!(original.position, Position::Unknown);
    assert_eq!(original.object, None);
    assert_eq!(original.repair, None);
    assert_eq!(document.diagnostics(), []);
    if let Some(provider) = pagura::provider::default_provider(document.limits()) {
        let error = refused(PdfFont::load_with_provider(
            &document,
            dictionary,
            Some(provider.as_ref()),
        ))?;
        assert_eq!(error.feature(), original.feature());
        assert_eq!(error.message, "font has no safe available substitute");
        assert_eq!(error.position, Position::Unknown);
        assert_eq!(error.object, None);
        assert_eq!(error.repair, None);
        assert_eq!(document.diagnostics(), []);
    }
    Ok(())
}

#[test]
fn all_strict_renderers_preserve_the_original_complete_errors_and_valid_pixels() -> Result<()> {
    // Captured independently on frozen cc99 for these exact64 PDF bytes:
    // gont-original-strict-01. Every refused font selection is the original
    // Tf at stream4 byte9/font5; canonical malformed data keeps its Syntax.
    for case in Manifest::load(AREA).cases() {
        let context = pagura::render_context(Arc::new(open(case.name(), Recovery::Strict)?));
        let index = usize::try_from(case.get("page").and_then(Json::as_u64).unwrap_or(0))
            .map_err(|_| Error::syntax("fixture page index overflow"))?;
        let result =
            context.render_page(context.document().page(index)?, &RenderOptions::default());
        if case.get("strict_valid").and_then(Json::as_bool) == Some(true) {
            assert_eq!(
                ppm(&result?),
                fixture(AREA, &format!("{}.ppm", case.name())),
                "{}",
                case.id()
            );
        } else {
            let original = if case.name() == "descriptor-canonical-malformed" {
                Error::syntax("expected a dictionary")
            } else {
                Error::unsupported(
                    pagura_text::features::NON_EMBEDDED,
                    if pagura::provider::BUNDLED {
                        "font has no safe available substitute"
                    } else {
                        "font program is not embedded"
                    },
                )
            }
            .at(Position::Stream {
                object: ObjectId::new(4, 0),
                offset: 9,
            })
            .in_object(ObjectId::new(5, 0));
            assert_eq!(refused(result)?, original, "{}", case.id());
            assert_eq!(
                refused(
                    context.render_page(context.document().page(index)?, &RenderOptions::default())
                )?,
                original,
                "{}",
                case.id()
            );
            assert_eq!(context.document().diagnostics(), [], "{}", case.id());
        }
    }
    Ok(())
}

#[test]
fn a_provider_error_never_becomes_descriptor_recovery() -> Result<()> {
    let document = open("descriptor-alias", Recovery::Repair)?;
    let mut dictionary = dictionary(&document)?.clone();
    dictionary.insert(
        pagura::Name::new(b"BaseFont"),
        pagura::Value::name(b"Helvetica"),
    );
    assert_eq!(
        refused(PdfFont::load_with_provider(
            &document,
            &dictionary,
            Some(&FailedProvider)
        ))?,
        provider_error()
    );
    assert_eq!(document.diagnostics(), []);
    Ok(())
}

#[test]
fn original_font_and_decode_budgets_propagate_without_missing_glyph_caching() -> Result<()> {
    for limits in [
        pagura::Limits {
            font_work: 0,
            ..pagura::Limits::default()
        },
        pagura::Limits {
            font_entries: 0,
            ..pagura::Limits::default()
        },
        pagura::Limits {
            path_segments: 0,
            ..pagura::Limits::default()
        },
        pagura::Limits {
            decoded_bytes: 1,
            ..pagura::Limits::default()
        },
    ] {
        let document = Document::open(
            fixture(AREA, "selection-named-windows-holes.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )?;
        for _ in 0..2 {
            let result = PdfFont::load(&document, dictionary(&document)?)
                .and_then(|font| font.decode(b"A", None));
            assert_eq!(refused(result)?.kind, ErrorKind::Limit);
        }
    }
    Ok(())
}

#[test]
fn descriptor_bbox_and_pdf_widths_determine_text_quads() -> Result<()> {
    for name in [
        "selection-descriptor-bbox-witness",
        "selection-canonical-descriptor-bbox-witness",
    ] {
        let context = RenderContext::new(Arc::new(open(name, Recovery::Repair)?));
        let page = context.document().page(0)?;
        let text = context.text_page(page)?;
        assert_eq!(text.content_text(), "AA");
        assert_eq!(text.glyphs().len(), 2);
        for (index, glyph) in text.glyphs().iter().enumerate() {
            let x = if index == 0 { 4.0 } else { 14.0 };
            let expected = [(x, 0.0), (x + 10.0, 0.0), (x + 10.0, 20.0), (x, 20.0)];
            for (point, (x, y)) in glyph.quad.0.iter().zip(expected) {
                assert_eq!((point.x, point.y), (x, y));
            }
            assert_eq!(glyph.advance_end.x - glyph.origin.x, 10.0);
        }
        assert_eq!(
            ppm(&context.render_page(page, &RenderOptions::default())?),
            fixture(AREA, &format!("{name}.ppm"))
        );
    }
    for (name, kind) in [
        ("selection-invalid-supplied-bbox", ErrorKind::Syntax),
        ("selection-cyclic-supplied-bbox", ErrorKind::Limit),
    ] {
        let context = RenderContext::new(Arc::new(open(name, Recovery::Repair)?));
        let page = context.document().page(0)?;
        assert_eq!(refused(context.text_page(page))?.kind, kind);
        assert_eq!(
            ppm(&context.render_page(page, &RenderOptions::default())?),
            fixture(AREA, &format!("{name}.ppm"))
        );
    }
    let document = open("selection-legacy-invalid-bbox", Recovery::Repair)?;
    assert_eq!(
        pagura::text_page(&document, document.page(0)?)?
            .glyphs()
            .len(),
        2
    );
    Ok(())
}
