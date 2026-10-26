use super::*;

#[test]
fn zero_cloud_intensity_preserves_the_original_outline() {
    for subtype in ["Square", "Circle", "Polygon", "FreeText"] {
        let extra = match subtype {
            "Polygon" => "/Vertices [8 8 24 8 24 24 8 24]",
            "FreeText" => "/DA (/Helvetica 12 Tf) /Contents ()",
            _ => "",
        };
        let source = format!("/Subtype /{subtype} {extra}");
        let plain = generate(
            format!("<< {source} >>").as_bytes(),
            rect(0.0, 0.0, 32.0, 32.0),
        )
        .unwrap();
        for effect in [
            "/BE << /S /C >>",
            "/BE << /S /C /I 0 >>",
            "/BE << /S /C /I null >>",
        ] {
            assert_eq!(
                generate(
                    format!("<< {source} {effect} >>").as_bytes(),
                    rect(0.0, 0.0, 32.0, 32.0)
                )
                .unwrap(),
                plain
            );
        }
    }
}

#[test]
fn cloudy_strokes_preserve_width_dash_and_alpha() {
    let stream = generate(b"<< /Subtype /Square /RD [8 8 8 8] /BE << /S /C /I 1 >> /BS << /W 2 /S /D /D [3 1] >> /CA 0.5 >>", rect(0.0, 0.0, 32.0, 32.0)).unwrap().unwrap();
    let content = String::from_utf8_lossy(stream.content());
    assert!(content.contains("2 w\n[3 1] 0 d\n"));
    assert!(content.starts_with("/StrokeAlpha gs\n"));
    assert_eq!(content.matches(" c\n").count(), 4);
    assert!(content.ends_with("h\nS\n"));
}

#[test]
fn malformed_cloud_intensity_is_checked_before_empty_output() {
    for effect in ["/I -1", "/I 3", "/I /Bad"] {
        assert_eq!(
            generate(
                format!("<< /Subtype /Square /C [] /BS << /W 0 >> /BE << /S /C {effect} >> >>")
                    .as_bytes(),
                rect(0.0, 0.0, 32.0, 32.0)
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn cloud_geometry_work_storage_and_output_are_bounded() {
    let annotation = dictionary(b"<< /Subtype /Square /BE << /S /C /I 1 >> >>");
    for limits in [
        Limits {
            path_segments: 8,
            ..Limits::default()
        },
        Limits {
            operations: 8,
            ..Limits::default()
        },
        Limits {
            document_bytes: 1_600,
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
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(0.0, 0.0, 100.0, 100.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}
