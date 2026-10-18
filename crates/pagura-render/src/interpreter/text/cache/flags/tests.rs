use super::*;
use pagura_core::{ObjectId, Recovery};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;
use pagura_text::GlyphShape;

#[test]
fn generated_flags_have_strict_tags_exact_pixels_and_text() {
    for name in [
        "both",
        "both-ambiguous",
        "neither-ambiguous",
        "neither",
        "symbolic-latin",
        "nonsymbolic-symbol",
        "valid-latin",
        "symbol-post-named",
        "valid-symbol",
        "ignored-encoding-cycle",
        "ambiguous-symbol",
        "unicode-no-encoding",
        "type1-symbolic-latin",
        "type1-nonsymbolic-symbol",
        "nonsymbolic-symbol-encoding",
        "symbolic-latin-differences",
        "nonsymbolic-latin-differences",
        "type1-nonsymbolic-override",
        "both-post",
        "symbolic-latin-post",
        "post-agl-components",
        "post-tounicode-priority",
        "post-encoding-priority",
        "post-unknown-name",
        "post-nondingbats-a12",
        "post-nondingbats-a19",
        "post-zapfdingbats-a12",
        "post-zapfdingbats-a19",
        "post-dingbats-a19",
        "post-nondingbats-a12-encoding",
    ] {
        let bytes = fixture("font", &format!("descriptor-{name}.pdf"));
        let expected =
            String::from_utf8(fixture("font", &format!("descriptor-{name}.expect"))).unwrap();
        let text = expected
            .lines()
            .find_map(|line| line.strip_prefix("text "))
            .unwrap();
        let repaired = fixture("font", &format!("descriptor-{name}.strict")) != b"valid\n";
        check_strict(bytes.clone(), name, repaired);
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        let original = document
            .object(ObjectId::new(6, 0))
            .unwrap()
            .value()
            .clone();
        let dictionary = document
            .object(ObjectId::new(5, 0))
            .unwrap()
            .dictionary()
            .unwrap();
        let font = load(&document, dictionary, None, Position::Buffer(0), false).unwrap();
        let glyph = font.decode(&[65]).unwrap().remove(0);
        assert_eq!(glyph.advance, 0.6, "{name}");
        let decoded = expected
            .lines()
            .find_map(|line| line.strip_prefix("decoded-text "))
            .unwrap_or(text);
        assert_eq!(glyph.unicode.as_deref().unwrap_or(""), decoded, "{name}");
        let GlyphShape::Outline { glyph, .. } = glyph.shape else {
            panic!("{name}")
        };
        let expected_glyph: u32 = expected
            .lines()
            .find_map(|line| line.strip_prefix("glyph "))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(glyph.0, expected_glyph, "{name}");
        let expected_symbolic = expected
            .lines()
            .find_map(|line| line.strip_prefix("symbolic "))
            .unwrap()
            == "1";
        let descriptor = descriptor::Descriptor::find(&document, dictionary)
            .unwrap()
            .unwrap();
        let flags = descriptor.flags(&document).unwrap().unwrap();
        let corrected = descriptor.corrected(&document, flags & 4 != 0).unwrap();
        let effective = descriptor::Descriptor::find(&document, &corrected)
            .unwrap()
            .unwrap()
            .flags(&document)
            .unwrap()
            .unwrap();
        assert_eq!(effective & 4 != 0, expected_symbolic, "{name}");
        assert_ne!(effective & 4 != 0, effective & 32 != 0, "{name}");
        assert_eq!(effective & !0x24, flags & !0x24, "{name}");
        let (list, extracted) =
            crate::compile_page_with_text(&document, document.page(0).unwrap(), None).unwrap();
        assert_eq!(extracted.glyphs().len(), 1, "{name}");
        assert_eq!(
            extracted.glyphs()[0].unicode.as_deref().unwrap_or(""),
            text,
            "{name}"
        );
        check_result(&document, &original, &list, name, repaired);
    }
}

fn check_strict(bytes: Vec<u8>, name: &str, repaired: bool) {
    let strict = Document::open(
        bytes,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let result = crate::compile_page(&strict, strict.page(0).unwrap());
    if repaired {
        let error = result.unwrap_err();
        assert_eq!(
            error.repair,
            Some(pagura_font::repairs::DESCRIPTOR_FLAGS),
            "{name}: {error}"
        );
        assert_eq!(error.object, Some(ObjectId::new(6, 0)), "{name}");
    } else {
        result.unwrap();
    }
    assert!(strict.diagnostics().is_empty(), "{name}");
}

fn check_result(
    document: &Document,
    original: &Value,
    list: &crate::DisplayList,
    name: &str,
    repaired: bool,
) {
    let pixels = list.render(&crate::RenderOptions::default()).unwrap();
    let oracle = fixture("font", &format!("descriptor-{name}.ppm"));
    let rgb = oracle.strip_prefix(b"P6\n16 16\n255\n").unwrap();
    assert_eq!((pixels.width(), pixels.height()), (16, 16));
    for (pixel, expected) in pixels
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgb.as_chunks::<3>().0)
    {
        assert_eq!(&pixel[..3], expected, "{name}");
        assert_eq!(pixel[3], 255);
    }
    assert_eq!(
        document.object(ObjectId::new(6, 0)).unwrap().value(),
        original,
        "{name}"
    );
    assert_eq!(
        document
            .diagnostics()
            .iter()
            .any(|d| d.repair() == Some(pagura_font::repairs::DESCRIPTOR_FLAGS)),
        repaired,
        "{name}"
    );
    assert!(
        document
            .diagnostics()
            .iter()
            .all(|d| d.object == Some(ObjectId::new(6, 0))),
        "{name}"
    );
}
