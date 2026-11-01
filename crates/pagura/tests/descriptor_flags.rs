#![forbid(unsafe_code)]
//! Descriptor flag normalization through rendering and extraction: strict
//! rejection, exact pixels and text of the generated `font/descriptor-*`
//! vectors (9.8.2, Table 121). The source objects stay unchanged.
use pagura::{Document, ObjectId, OpenOptions, Position, Recovery};
use pagura_testkit::{Manifest, fixture};
use pagura_text::repairs::DESCRIPTOR_FLAGS;

const DESCRIPTOR: ObjectId = ObjectId::new(6, 0);

#[test]
fn descriptor_vectors_render_exact_pixels_and_text() {
    let manifest = Manifest::load("font");
    let names: Vec<&str> = manifest
        .cases()
        .map(|case| case.name())
        .filter(|name| name.starts_with("descriptor-"))
        .collect();
    assert_eq!(names.len(), 30);
    for name in names {
        let pdf = fixture("font", &format!("{name}.pdf"));
        let repaired = fixture("font", &format!("{name}.strict")) != b"valid\n";
        let expect = String::from_utf8(fixture("font", &format!("{name}.expect"))).unwrap();
        let text = expect
            .lines()
            .find_map(|line| line.strip_prefix("text "))
            .unwrap();

        let strict = Document::open(
            pdf.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let result = pagura_render::record_page(&strict, strict.page(0).unwrap(), None);
        if repaired {
            let error = result.unwrap_err();
            assert_eq!(error.repair, Some(DESCRIPTOR_FLAGS), "{name}: {error}");
            assert_eq!(error.object, Some(DESCRIPTOR), "{name}");
        } else {
            result.unwrap();
        }
        assert!(strict.diagnostics().is_empty(), "{name}");

        let document = Document::open(pdf, &OpenOptions::default()).unwrap();
        let original = document.object(DESCRIPTOR).unwrap().value().clone();
        let page = document.page(0).unwrap();
        let extracted = pagura_render::text_page(&document, page, None).unwrap();
        assert_eq!(extracted.glyphs().len(), 1, "{name}");
        assert_eq!(
            extracted.glyphs()[0].unicode.as_deref().unwrap_or(""),
            text,
            "{name}"
        );
        let pixels =
            pagura_render::render_page(&document, page, &pagura::RenderOptions::default(), None)
                .unwrap();
        let oracle = fixture("font", &format!("{name}.ppm"));
        let rgb = oracle.strip_prefix(b"P6\n16 16\n255\n").unwrap();
        assert_eq!((pixels.width(), pixels.height()), (16, 16), "{name}");
        for (pixel, expected) in pixels
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(rgb.as_chunks::<3>().0)
        {
            assert_eq!(&pixel[..3], expected, "{name}");
            assert_eq!(pixel[3], 255, "{name}");
        }
        assert_eq!(
            document.object(DESCRIPTOR).unwrap().value(),
            &original,
            "{name}"
        );
        let diagnostics = document.diagnostics();
        assert_eq!(
            diagnostics
                .iter()
                .any(|d| d.repair() == Some(DESCRIPTOR_FLAGS)),
            repaired,
            "{name}"
        );
        assert!(
            diagnostics.iter().all(|d| d.object == Some(DESCRIPTOR)),
            "{name}"
        );
        // The repair is located at the content-stream operator that selected
        // the font.
        assert!(
            diagnostics
                .iter()
                .all(|d| matches!(d.position, Position::Stream { .. })),
            "{name}: {diagnostics:?}"
        );
    }
}
