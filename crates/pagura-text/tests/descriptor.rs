#![forbid(unsafe_code)]
//! Descriptor flag normalization (9.8.2, Table 121) on the generated
//! `font/descriptor-*` vectors: strict tags, glyphs, advances and text.
use pagura_core::{Limits, ObjectId, Recovery, Result};
use pagura_document::{Document, OpenOptions};
use pagura_font::{FontFormat, FontProgram};
use pagura_testkit::{Manifest, fixture};
use pagura_text::{
    CidToUnicode, FontKind, GlyphShape, PdfFont, PredefinedCMap, Provider, StandardFont, repairs,
};
use std::{collections::BTreeMap, sync::Arc};

/// One `font/descriptor-*` vector: its PDF and the `.expect` oracle lines.
struct Vector {
    name: String,
    pdf: Vec<u8>,
    repaired: bool,
    expect: String,
}

impl Vector {
    fn all() -> Vec<Self> {
        Manifest::load("font")
            .cases()
            .filter(|case| case.name().starts_with("descriptor-"))
            .map(|case| {
                let name = case.name().to_owned();
                Self {
                    pdf: fixture("font", &format!("{name}.pdf")),
                    repaired: fixture("font", &format!("{name}.strict")) != b"valid\n",
                    expect: String::from_utf8_lossy(&fixture("font", &format!("{name}.expect")))
                        .into_owned(),
                    name,
                }
            })
            .collect()
    }

    fn line(&self, key: &str) -> Option<&str> {
        self.expect
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
    }

    fn open(&self, recovery: Recovery) -> Result<Document> {
        Document::open(
            self.pdf.clone(),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
    }
}

/// The descriptor of every vector.
const DESCRIPTOR: ObjectId = ObjectId::new(6, 0);

#[test]
fn contradictory_or_missing_flags_are_repaired_and_valid_pairs_kept() -> Result<()> {
    let vectors = Vector::all();
    assert_eq!(vectors.len(), 30);
    for vector in vectors {
        let name = &vector.name;
        let strict = vector.open(Recovery::Strict)?;
        let font = strict.object(ObjectId::new(5, 0))?.dictionary().unwrap();
        match PdfFont::load(&strict, font) {
            Err(error) if vector.repaired => {
                assert_eq!(error.repair, Some(repairs::DESCRIPTOR_FLAGS), "{name}");
                assert_eq!(error.object, Some(DESCRIPTOR), "{name}");
            }
            Ok(_) if !vector.repaired => {}
            other => panic!("{name}: {other:?}"),
        }
        assert!(strict.diagnostics().is_empty(), "{name}");
        let document = vector.open(Recovery::Repair)?;
        let font = document.object(ObjectId::new(5, 0))?.dictionary().unwrap();
        let glyph = PdfFont::load(&document, font)?.decode(&[65])?.remove(0);
        assert_eq!(glyph.advance, 0.6, "{name}");
        let text = vector.line("text").unwrap();
        let decoded = vector.line("decoded-text").unwrap_or(text);
        assert_eq!(glyph.unicode.as_deref().unwrap_or(""), decoded, "{name}");
        let GlyphShape::Outline { glyph, .. } = glyph.shape else {
            panic!("{name}")
        };
        assert_eq!(glyph.0.to_string(), vector.line("glyph").unwrap(), "{name}");
        let diagnostics = document.diagnostics();
        assert_eq!(
            diagnostics
                .iter()
                .any(|d| d.repair() == Some(repairs::DESCRIPTOR_FLAGS)),
            vector.repaired,
            "{name}"
        );
        assert!(
            diagnostics.iter().all(|d| d.object == Some(DESCRIPTOR)),
            "{name}"
        );
    }
    Ok(())
}

/// A provider whose CID substitute is a fixture program without collection
/// Unicode data: enough to take the substitution path.
struct Collection(Arc<FontProgram>);

impl Provider for Collection {
    fn cid_font(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        Ok(Some(Arc::clone(&self.0)))
    }
    fn standard_font(&self, _: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        Ok(None)
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<PredefinedCMap>> {
        Ok(None)
    }
    fn cid_to_unicode(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<CidToUnicode>>> {
        Ok(Some(Arc::new(CidToUnicode::new(
            BTreeMap::new(),
            &Limits::default(),
        )?)))
    }
}

#[test]
fn substituted_cid_fonts_still_repair_contradictory_descriptor_flags() -> Result<()> {
    // The CID substitute never reads the flags, but Symbolic and Nonsymbolic
    // both set is still the descriptor's repair (9.8.2, Table 121).
    let provider = Collection(Arc::new(FontProgram::parse(
        fixture("font", "truetype-cmap4.font"),
        FontFormat::TrueType,
        &Limits::default(),
    )?));
    let name = "repair-cid-substitute-flags";
    let open = |recovery| {
        Document::open(
            fixture("text", &format!("{name}.pdf")),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
    };
    let font = |document: &Document| -> Result<PdfFont> {
        let dictionary = document.object(ObjectId::new(4, 0))?.dictionary().unwrap();
        PdfFont::load_with_provider(document, dictionary, Some(&provider))
    };
    let strict = open(Recovery::Strict)?;
    let error = font(&strict).unwrap_err();
    assert_eq!(error.repair, Some(repairs::DESCRIPTOR_FLAGS), "{error}");
    assert_eq!(error.object, Some(ObjectId::new(6, 0)));
    let repaired = open(Recovery::Repair)?;
    assert_eq!(font(&repaired)?.kind(), FontKind::Type0);
    assert_eq!(
        fixture("text", &format!("{name}.repair")),
        format!("{}\n", repairs::DESCRIPTOR_FLAGS.as_str()).into_bytes()
    );
    let flags: Vec<_> = repaired
        .diagnostics()
        .into_iter()
        .filter(|item| item.repair() == Some(repairs::DESCRIPTOR_FLAGS))
        .map(|item| item.object)
        .collect();
    assert_eq!(flags, [Some(ObjectId::new(6, 0))]);
    Ok(())
}
