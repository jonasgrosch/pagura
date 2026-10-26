use super::*;
use pagura_core::{Name, Recovery};

#[test]
fn empty_markup_quads_do_not_emit_device_hairlines_or_hide_later_geometry() {
    let area = rect(0.0, 0.0, 32.0, 32.0);
    for subtype in ["Highlight", "Underline", "StrikeOut", "Squiggly"] {
        let make = |quads: &str| {
            dictionary(format!("<< /Subtype /{subtype} /QuadPoints [{quads}] >>").as_bytes())
        };
        let document = document();
        let normal = "4 8 28 8 28 20 4 20";
        let expected = annotation_appearance(&document, &make(normal), area).unwrap();
        for empty in ["4 8 28 8 28 8 4 8", "4 4 4 4 4 4 4 4", "4 4 8 8 8 8 4 4"] {
            assert!(
                annotation_appearance(&document, &make(empty), area)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                annotation_appearance(&document, &make(&format!("{empty} {normal}")), area)
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            annotation_appearance(&document, &make("4 8 28 8 28 8 4 /Bad"), area)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn alternate_markup_quad_order_is_reported_and_strict_mode_refuses_it() {
    for subtype in ["Highlight", "Underline", "StrikeOut", "Squiggly"] {
        let area = rect(0.0, 0.0, 32.0, 32.0);
        let normal = dictionary(
            format!("<< /Subtype /{subtype} /QuadPoints [4 8 28 8 28 20 4 20] >>").as_bytes(),
        );
        let alternate = dictionary(
            format!("<< /Subtype /{subtype} /QuadPoints [4 20 28 20 4 8 28 8] >>").as_bytes(),
        );
        let repaired = document();
        let expected = annotation_appearance(&repaired, &normal, area).unwrap();
        assert_eq!(repaired.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
        assert_eq!(
            annotation_appearance(&repaired, &alternate, area).unwrap(),
            expected
        );
        assert_eq!(
            repaired
                .diagnostics()
                .iter()
                .map(pagura_core::Diagnostic::tag)
                .collect::<Vec<_>>(),
            [crate::repairs::QUAD_ORDER.as_str()]
        );
        let strict = Document::open(
            fixture("classic", "rectangle.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert!(annotation_appearance(&strict, &normal, area).is_ok());
        let error = annotation_appearance(&strict, &alternate, area).unwrap_err();
        assert_eq!(error.repair, Some(crate::repairs::QUAD_ORDER));
    }
}

#[test]
fn markup_geometry_validates_numeric_range_before_empty_color() {
    let document = document();
    for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MAX] {
        let mut annotation = dictionary(b"<< /Subtype /Highlight /C [] >>");
        annotation.insert(
            Name::new(b"QuadPoints"),
            Value::Array(vec![
                Value::Real(value),
                Value::Integer(8),
                Value::Integer(28),
                Value::Integer(8),
                Value::Integer(28),
                Value::Integer(20),
                Value::Integer(4),
                Value::Integer(20),
            ]),
        );
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit,
        );
    }
}

#[test]
fn markup_bounds_quads_wave_segments_storage_and_output() {
    for limits in [
        Limits {
            path_segments: 3,
            ..Limits::default()
        },
        Limits {
            operations: 3,
            ..Limits::default()
        },
        Limits {
            render_bytes: 1,
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
        for subtype in ["Highlight", "Squiggly"] {
            let annotation = dictionary(
                format!("<< /Subtype /{subtype} /QuadPoints [4 8 28 8 28 20 4 20] >>").as_bytes(),
            );
            assert_eq!(
                annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit
            );
        }
    }
    // Finite, extremely thin quads must fail the wave work bound before a cast
    // or a large allocation, even when the requested color paints nothing.
    let annotation = dictionary(
        b"<< /Subtype /Squiggly /C [] /QuadPoints [4 0 28 0 28 0.000000000001 4 0.000000000001] >>",
    );
    assert_eq!(
        annotation_appearance(&document(), &annotation, rect(0.0, 0.0, 32.0, 32.0))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
