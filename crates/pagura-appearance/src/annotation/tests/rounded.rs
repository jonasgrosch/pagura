use super::*;

#[test]
fn rounded_border_limits_apply_before_invisible_output() {
    let annotation = dictionary(b"<< /Subtype /Square /Border [6 6 2] /C [] >>");
    for limits in [
        Limits {
            path_segments: 23,
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
fn corner_types_and_numeric_range_are_validated() {
    let document = document();
    let area = rect(4.0, 4.0, 28.0, 28.0);
    for entries in ["/Border [-1 6 2]", "/Border [/X 6 2]", "/Border [6 6 /X]"] {
        assert_eq!(
            generate(
                format!("<< /Subtype /Square /C [] {entries} >>").as_bytes(),
                area
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
    let mut annotation = dictionary(b"<< /Subtype /Square /C [] >>");
    annotation.insert(
        pagura_core::Name::new(b"Border"),
        Value::Array(vec![
            Value::Real(f64::INFINITY),
            Value::Integer(6),
            Value::Integer(2),
        ]),
    );
    assert_eq!(
        annotation_appearance(&document, &annotation, area)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    annotation.insert(
        pagura_core::Name::new(b"Border"),
        Value::Array(vec![
            Value::Integer(6),
            Value::Integer(6),
            Value::Integer(2),
        ]),
    );
    annotation.insert(
        pagura_core::Name::new(b"C"),
        Value::Array(vec![Value::Integer(0)]),
    );
    assert_eq!(
        annotation_appearance(&document, &annotation, rect(-f64::MAX, 4.0, f64::MAX, 28.0))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
