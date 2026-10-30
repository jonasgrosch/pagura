use super::*;

#[test]
fn free_text_uses_explicit_fonts_and_its_own_resources() {
    let document = document();
    let annotation = dictionary(
        b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents (HI) /BS << /W 0 >> >>",
    );
    let area = rect(4.0, 4.0, 124.0, 44.0);
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
    );
    let provider = pagura_data::Bundled::default();
    let fonts = ProviderFonts::new(Some(&provider), &document.limits());
    let stream = annotation_appearance_with_fonts(&document, &annotation, area, &fonts)
        .unwrap()
        .unwrap();
    let resources = stream
        .dictionary()
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .unwrap();
    let entries = resources
        .get(b"Font")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(b"Helvetica"));
    assert_eq!(
        stream
            .dictionary()
            .get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(area)
    );
}

#[test]
fn empty_free_text_validates_style_and_needs_no_font_provider() {
    let area = rect(4.0, 4.0, 124.0, 44.0);
    assert!(
        generate(
            b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents () /BS << /W 0 >> >>",
            area
        )
        .unwrap()
        .is_none()
    );
    for entry in [
        "/DA 42",
        "/DA (/Helvetica 12 Tf) /Q 3",
        "/DA (/Helvetica 12 Tf) /Contents 42",
        "/DA (/Helvetica 12 Tf) /RD [1 2 3]",
    ] {
        assert_eq!(
            generate(
                format!("<< /Subtype /FreeText /BS << /W 0 >> {entry} >>").as_bytes(),
                area
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
    assert_eq!(
        generate(
            b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) >>",
            rect(-f64::MAX, 0.0, f64::MAX, 32.0)
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}

#[test]
fn free_text_respects_work_geometry_mapping_and_output_budgets() {
    let provider = pagura_data::Bundled::default();
    let annotation = dictionary(
        b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents (HI) /BS << /W 0 >> >>",
    );
    for limits in [
        Limits {
            operations: 260,
            ..Limits::default()
        },
        Limits {
            path_segments: 11,
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
        assert_eq!(
            annotation_appearance_with_fonts(
                &document,
                &annotation,
                rect(4.0, 4.0, 124.0, 44.0),
                &fonts
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn callout_geometry_is_validated_with_empty_text_and_no_border() {
    let document = document();
    let area = rect(0.0, 0.0, 32.0, 32.0);
    let mut annotation = dictionary(b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents () /IT /FreeTextCallout /BS << /W 0 >> >>");
    annotation.insert(
        pagura_core::Name::new(b"CL"),
        Value::Array(vec![Value::Integer(0); 10_000]),
    );
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    annotation.insert(
        pagura_core::Name::new(b"CL"),
        Value::Array(vec![
            Value::Real(-f64::MAX),
            Value::Integer(0),
            Value::Real(f64::MAX),
            Value::Integer(0),
        ]),
    );
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let limits = Limits {
        path_segments: 59,
        ..Limits::default()
    };
    let document = Document::open(
        fixture("classic", "rectangle.pdf"),
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let annotation = dictionary(b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents () /IT /FreeTextCallout /CL [4 4 12 12] /BS << /W 0 >> >>");
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
