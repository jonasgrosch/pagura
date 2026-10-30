use super::*;
use pagura_core::Name;

#[test]
fn caret_and_paragraph_need_no_font_and_keep_the_original_clip() {
    let document = document();
    let area = rect(4.0, 6.0, 28.0, 30.0);
    for symbol in ["None", "P"] {
        let annotation = dictionary(
            format!("<< /Subtype /Caret /Sy /{symbol} /RD [2 4 6 8] /CA 0.5 >>").as_bytes(),
        );
        let appearance = annotation_appearance(&document, &annotation, area)
            .unwrap()
            .unwrap();
        assert_eq!(
            appearance
                .dictionary()
                .get(b"BBox")
                .and_then(Value::as_array)
                .and_then(Rect::from_array),
            Some(area)
        );
        let resources = appearance
            .dictionary()
            .get(b"Resources")
            .and_then(Value::as_dictionary)
            .unwrap();
        assert!(!resources.contains_key(b"Font"));
        assert!(!appearance.dictionary().contains_key(b"Matrix"));
    }
    assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
}

#[test]
fn rectangle_insets_use_fixed_storage_and_reject_numeric_overflow() {
    let document = document();
    for subtype in ["Caret", "Square", "Circle"] {
        let mut annotation = dictionary(format!("<< /Subtype /{subtype} /C [] >>").as_bytes());
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MAX] {
            annotation.insert(
                Name::new(b"RD"),
                Value::Array(vec![
                    Value::Real(value),
                    Value::Integer(0),
                    Value::Real(value),
                    Value::Integer(0),
                ]),
            );
            assert_eq!(
                annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit
            );
        }
        annotation.insert(
            Name::new(b"RD"),
            Value::Array(vec![Value::Integer(0); 10_000]),
        );
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        annotation.remove(b"RD");
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(-f64::MAX, 0.0, f64::MAX, 32.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn caret_generation_bounds_fixed_geometry_and_content() {
    for limits in [
        Limits {
            path_segments: 15,
            ..Limits::default()
        },
        Limits {
            operations: 15,
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
        let annotation = dictionary(b"<< /Subtype /Caret /Sy /P >>");
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}
