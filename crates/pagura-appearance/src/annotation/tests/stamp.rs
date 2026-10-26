use super::*;

#[test]
fn stamps_require_explicit_fonts_and_keep_private_resources() {
    let document = document();
    let area = rect(4.0, 4.0, 124.0, 28.0);
    let annotation = dictionary(b"<< /Subtype /Stamp /Name /Final >>");
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
    );
    let provider = pagura_data::Bundled::default();
    let fonts = ProviderFonts::new(Some(&provider), &document.limits());
    let appearance = annotation_appearance_with_fonts(&document, &annotation, area, &fonts)
        .unwrap()
        .unwrap();
    let font = appearance
        .dictionary()
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .and_then(|resources| resources.get(b"Font"))
        .and_then(Value::as_dictionary)
        .and_then(|fonts| fonts.get(b"StampText"))
        .and_then(Value::as_dictionary)
        .unwrap();
    assert!(font.is_name(b"BaseFont", b"Helvetica-Bold"));
    assert!(font.is_name(b"Encoding", b"WinAnsiEncoding"));
    assert_eq!(
        appearance
            .dictionary()
            .get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(area)
    );
}

#[test]
fn invisible_stamps_still_validate_names_intents_and_numeric_bounds() {
    let area = rect(0.0, 0.0, 32.0, 32.0);
    assert!(
        generate(b"<< /Subtype /Stamp /C [] >>", area)
            .unwrap()
            .is_none()
    );
    for entries in ["/Name 42", "/IT 42"] {
        assert_eq!(
            generate(
                format!("<< /Subtype /Stamp /C [] {entries} >>").as_bytes(),
                area
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
    assert_eq!(
        generate(
            b"<< /Subtype /Stamp /C [] >>",
            rect(-f64::MAX, 0.0, f64::MAX, 32.0)
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}

#[test]
fn stamp_text_frame_and_output_respect_limits() {
    let provider = pagura_data::Bundled::default();
    for limits in [
        Limits {
            operations: 260,
            ..Limits::default()
        },
        Limits {
            path_segments: 3,
            ..Limits::default()
        },
        Limits {
            document_bytes: 16_000,
            ..Limits::default()
        },
        Limits {
            stream_bytes: 16,
            ..Limits::default()
        },
    ] {
        let document = Document::open(
            fixture("classic", "rectangle.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let fonts = ProviderFonts::new(Some(&provider), &document.limits());
        let annotation = dictionary(b"<< /Subtype /Stamp /Name /Final >>");
        assert_eq!(
            annotation_appearance_with_fonts(
                &document,
                &annotation,
                rect(4.0, 4.0, 124.0, 28.0),
                &fonts
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
    }
}
