//! Descriptor flag normalization (9.8.2, Table 121) on the generated
//! `font/descriptor-*` vectors: strict tags, glyphs, advances and text.
use pagura_core::{ObjectId, Recovery, Result};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::{Manifest, fixture};
use pagura_text::{GlyphShape, PdfFont, repairs};

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
fn contradictory_or_missing_flags_are_repaired_and_valid_pairs_kept()
-> pagura_document::source::ReadResult<()> {
    let vectors = Vector::all();
    assert_eq!(vectors.len(), 30);
    for vector in vectors {
        let name = &vector.name;
        let strict = vector.open(Recovery::Strict)?;
        let font = strict.object(ObjectId::new(5, 0))?.dictionary().unwrap();
        match PdfFont::load(&strict, font) {
            Err(error) if vector.repaired => {
                assert_eq!(
                    error.as_error().unwrap().repair,
                    Some(repairs::DESCRIPTOR_FLAGS),
                    "{name}"
                );
                assert_eq!(error.as_error().unwrap().object, Some(DESCRIPTOR), "{name}");
            }
            Ok(_) if !vector.repaired => {}
            other => panic!("{name}: {other:?}"),
        }
        assert!(strict.diagnostics().is_empty(), "{name}");
        let document = vector.open(Recovery::Repair)?;
        let font = document.object(ObjectId::new(5, 0))?.dictionary().unwrap();
        let loaded = PdfFont::load(&document, font)?;
        let unicode = loaded.unicode(&document, font, None)?;
        let glyph = loaded.decode(&[65], Some(&unicode))?.remove(0);
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
