use super::*;

#[test]
fn relief_limits_apply_before_transparent_output() {
    let annotation = dictionary(b"<< /Subtype /Square /BS << /S /B /W 2 >> /C [] >>");
    for limits in [
        Limits {
            path_segments: 15,
            ..Limits::default()
        },
        Limits {
            operations: 23,
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
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(4.0, 4.0, 28.0, 28.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn relief_opacity_has_private_resources_and_preserves_original_bounds() {
    let area = rect(4.0, 4.0, 28.0, 28.0);
    let stream = generate(
        b"<< /Subtype /Square /BS << /S /I /W 2 >> /C [0] /IC [1 0 0] /CA 1 /ca 0.25 /RD [2 4 6 2] >>",
        area,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        stream
            .dictionary()
            .get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(area)
    );
    let resources = stream
        .dictionary()
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .unwrap();
    let states = resources
        .get(b"ExtGState")
        .and_then(Value::as_dictionary)
        .unwrap();
    let fill = states
        .get(b"FillAlpha")
        .and_then(Value::as_dictionary)
        .unwrap();
    let border = states
        .get(b"BorderAlpha")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert_eq!(fill.get(b"ca").and_then(Value::as_number), Some(0.25));
    assert_eq!(border.get(b"ca").and_then(Value::as_number), Some(1.0));
    assert!(!resources.contains_key(b"Font"));
    assert_eq!(
        generate(
            b"<< /Subtype /Link /BS << /S /B >> /C [0] >>",
            rect(-f64::MAX, 0.0, f64::MAX, 32.0)
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}
