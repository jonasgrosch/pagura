use super::*;
use pagura_core::{ErrorKind, Limits, syntax::Origin, syntax::Parser};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;

fn dictionary(text: &[u8]) -> Dictionary {
    Parser::new(text, Origin::Buffer, Limits::default())
        .parse()
        .unwrap()
        .as_dictionary()
        .unwrap()
        .clone()
}

fn document() -> Document {
    Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::from_corners(x0, y0, x1, y1).unwrap()
}

fn generate(text: &[u8], area: Rect) -> Result<Option<AppearanceStream>> {
    annotation_appearance(&document(), &dictionary(text), area)
}

#[test]
fn a_link_border_is_a_stroke_inside_its_rectangle_in_default_user_space() {
    let stream = generate(
        b"<< /Subtype /Link /BS << /W 2 /S /D /D [3 1] >> /C [0 0 1] /CA 0.5 >>",
        rect(10.0, 20.0, 30.0, 25.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "/StrokeAlpha gs\n0 0 1 RG\n2 w\n[3 1] 0 d\n11 21 18 3 re\nS\n"
    );
    let form = stream.dictionary();
    assert!(form.is_name(b"Subtype", b"Form"));
    assert!(!form.contains_key(b"Matrix"));
    assert_eq!(
        form.get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(rect(10.0, 20.0, 30.0, 25.0))
    );
    let states = form
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .and_then(|r| r.get(b"ExtGState"))
        .and_then(Value::as_dictionary)
        .unwrap();
    let alpha = states
        .get(b"StrokeAlpha")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert_eq!(alpha.get(b"CA"), Some(&Value::Real(0.5)));
    assert_eq!(states.len(), 1);
}

#[test]
fn opaque_marks_need_no_graphics_state_but_keep_their_own_resources() {
    let stream = generate(
        b"<< /Subtype /Square /IC [1 0 0] /Border [0 0 0] >>",
        rect(0.0, 0.0, 8.0, 4.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "1 0 0 rg\n0 0 8 4 re\nf\n"
    );
    // An empty Resources keeps page defaults such as DefaultRGB out.
    let resources = stream
        .dictionary()
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert!(resources.is_empty());
}

#[test]
fn rectangle_differences_inset_left_top_right_bottom_without_changing_placement() {
    let area = rect(10.0, 20.0, 40.0, 60.0);
    let stream = generate(
        b"<< /Subtype /Square /RD [2 4 6 8] /BS << /W 0 >> /IC [1 0 0] >>",
        area,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "1 0 0 rg\n12 28 22 28 re\nf\n"
    );
    assert_eq!(
        stream
            .dictionary()
            .get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(area)
    );
    assert!(!stream.dictionary().contains_key(b"Matrix"));
}

#[test]
fn plain_and_absent_border_effects_preserve_circle_and_square_geometry() {
    let area = rect(4.0, 8.0, 28.0, 24.0);
    for subtype in ["Circle", "Square"] {
        let expected = generate(
            format!("<< /Subtype /{subtype} /IC [1 0 0] >>").as_bytes(),
            area,
        )
        .unwrap();
        for extra in [
            "/RD null /BE null",
            "/RD [0 0 0 0] /BE << >>",
            "/BE << /S null >>",
            "/BE << /S /S /I /IgnoredWithoutCloudyEffect >>",
        ] {
            assert_eq!(
                generate(
                    format!("<< /Subtype /{subtype} /IC [1 0 0] {extra} >>").as_bytes(),
                    area,
                )
                .unwrap(),
                expected,
                "{subtype} {extra}"
            );
        }
    }
}

#[test]
fn shape_differences_and_effects_reject_malformed_geometry() {
    for subtype in ["Circle", "Square"] {
        for extra in [
            "/RD 42",
            "/RD []",
            "/RD [0 0 0]",
            "/RD [0 0 0 0 0]",
            "/RD [-1 0 0 0]",
            "/RD [12 0 12 0]",
            "/RD [0 8 0 8]",
            "/RD [0 /Bad 0 0]",
            "/BE 42",
            "/BE << /S 42 >>",
        ] {
            assert_eq!(
                generate(
                    format!("<< /Subtype /{subtype} {extra} >>").as_bytes(),
                    rect(4.0, 8.0, 28.0, 24.0),
                )
                .unwrap_err()
                .kind,
                ErrorKind::Syntax,
                "{subtype} {extra}"
            );
        }
    }
}

#[test]
fn fill_opacity_falls_back_to_stroke_opacity_and_is_read_only_for_fills() {
    let stream = generate(
        b"<< /Subtype /Square /IC [0.5] /CA 0.25 >>",
        rect(0.0, 0.0, 8.0, 8.0),
    )
    .unwrap()
    .unwrap();
    let content = String::from_utf8_lossy(stream.content()).into_owned();
    assert!(
        content.starts_with("/StrokeAlpha gs\n/FillAlpha gs\n0.5 g\n"),
        "{content}"
    );
    // `ca` is never read without a fill, so a bad one is no error.
    assert!(generate(b"<< /Subtype /Link /ca /Bad >>", rect(0.0, 0.0, 8.0, 8.0)).is_ok());
    let error = generate(
        b"<< /Subtype /Square /IC [0] /ca /Bad >>",
        rect(0.0, 0.0, 8.0, 8.0),
    )
    .unwrap_err();
    assert_eq!(error.message, "annotation parameter must be numeric");
}

#[test]
fn note_icons_are_scaled_from_their_16_unit_design_space() {
    let stream = generate(
        b"<< /Subtype /Text /Name /Insert >>",
        rect(4.0, 8.0, 12.0, 16.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "0.5 0 0 0.5 4 8 cm\n1 1 0 rg\n1 1 14 14 re\nf\n0 G\n1 w\n[] 0 d\n\
         1 1 14 14 re\n3 4 m\n8 12 l\n13 4 l\nS\n"
    );
}

#[test]
fn nothing_is_generated_where_nothing_is_drawn() {
    let area = rect(0.0, 0.0, 8.0, 8.0);
    for text in [
        b"<< /Subtype /Popup >>".as_slice(),
        b"<< /Subtype /Projection >>",
        b"<< /Subtype /Link /C [] >>",
        b"<< /Subtype /Link /BS << /W 0 >> >>",
        b"<< /Subtype /Link /Border [0 0 0] >>",
        b"<< /Subtype /Square /Border [0 0 0] >>",
        b"<< /Subtype /Widget /FT /Tx >>",
    ] {
        assert_eq!(
            generate(text, area).unwrap(),
            None,
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let point = rect(3.0, 3.0, 3.0, 3.0);
    assert_eq!(generate(b"<< /Subtype /Circle >>", point).unwrap(), None);
}

#[test]
fn unsupported_annotations_are_tagged() {
    let area = rect(0.0, 0.0, 8.0, 8.0);
    for text in [
        b"<< /Subtype /Stamp >>".as_slice(),
        b"<< /Subtype /Square /BE << /S /C >> >>",
        b"<< /Subtype /Link /Border [2 2 1] >>",
        b"<< /Subtype /Link /BS << /S /I >> >>",
        b"<< /Subtype /Widget /FT /Tx /V (value) >>",
        b"<< /Subtype /Widget /FT /Ch >>",
        b"<< /Subtype /Widget /FT /Tx /MK << /BC [0] /R 90 >> >>",
    ] {
        let error = generate(text, area).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(features::ANNOTATION_APPEARANCE),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let error = generate(b"<< /Rect [0 0 1 1] >>", area).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
}

#[test]
fn content_and_storage_budgets_bound_generation() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions {
            limits: Limits {
                stream_bytes: 16,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for subtype in [
        b"<< /Subtype /Text >>".as_slice(),
        b"<< /Subtype /Circle >>",
    ] {
        let error =
            annotation_appearance(&document, &dictionary(subtype), rect(0.0, 0.0, 8.0, 8.0))
                .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}

#[test]
fn unusable_dash_arrays_fail_at_generation() {
    let area = rect(0.0, 0.0, 8.0, 8.0);
    for text in [
        b"<< /Subtype /Link /BS << /S /D /D [-1 2] >> >>".as_slice(),
        b"<< /Subtype /Link /Border [0 0 1 [0 0]] >>",
        b"<< /Subtype /Widget /FT /Tx /MK << /BC [0] >> /Border [0 0 1 [0]] >>",
    ] {
        let error = generate(text, area).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Syntax, "invalid annotation dash pattern"),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

/// A rectangle without area gets no appearance, as fitting one into it
/// would scale it to nothing (12.5.5), but its entries are still read with
/// the errors of a rectangle with area.
#[test]
fn rectangles_without_area_get_no_appearance_after_validation() {
    let line = rect(2.0, 2.0, 2.0, 9.0);
    for text in [
        b"<< /Subtype /Link >>".as_slice(),
        b"<< /Subtype /Square /IC [1 0 0] >>",
        b"<< /Subtype /Circle /IC [1 0 0] >>",
        b"<< /Subtype /Text >>",
        b"<< /Subtype /Widget /FT /Tx /MK << /BC [0] >> >>",
    ] {
        assert_eq!(
            generate(text, line).unwrap(),
            None,
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    for text in [
        b"<< /Subtype /Link /BS << /W /Thick >> >>".as_slice(),
        b"<< /Subtype /Text /C [0 0] >>",
        b"<< /Subtype /Circle /IC [0 0] >>",
        b"<< /Subtype /Circle /BE << /S 42 >> >>",
        b"<< /Subtype /Widget /FT /Tx /V (value) >>",
    ] {
        let flat = generate(text, line).unwrap_err();
        let full = generate(text, rect(2.0, 2.0, 9.0, 9.0)).unwrap_err();
        assert_eq!(
            (flat.kind, flat.message),
            (full.kind, full.message),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn file_attachment_default_null_unknown_and_indirect_names_are_consistent() {
    let area = rect(8.0, 8.0, 24.0, 24.0);
    let expected = generate(b"<< /Subtype /FileAttachment /Name /PushPin >>", area).unwrap();
    for value in [
        b"<< /Subtype /FileAttachment >>".as_slice(),
        b"<< /Subtype /FileAttachment /Name null >>",
        b"<< /Subtype /FileAttachment /Name /Unrecognized >>",
        b"<< /Subtype /FileAttachment /FS 999 0 R >>",
    ] {
        assert_eq!(generate(value, area).unwrap(), expected);
    }
    let document = Document::open(
        fixture("file_annotations", "indirect-array-and-geometry.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let dictionary = dictionary(b"<< /Subtype /FileAttachment /Name 12 0 R >>");
    let generated = annotation_appearance(&document, &dictionary, area).unwrap();
    assert_eq!(
        generated,
        generate(b"<< /Subtype /FileAttachment /Name /Graph >>", area).unwrap()
    );
}

#[test]
fn file_attachment_icons_have_bounded_content_and_validate_visual_entries() {
    let area = rect(8.0, 8.0, 24.0, 24.0);
    for name in [b"Graph".as_slice(), b"PushPin", b"Paperclip", b"Tag"] {
        let mut annotation = Dictionary::new();
        annotation.insert(
            pagura_core::Name::new(b"Subtype"),
            Value::name(b"FileAttachment"),
        );
        annotation.insert(pagura_core::Name::new(b"Name"), Value::name(name));
        let document = Document::open(
            fixture("forms", "text-quadding-0.pdf"),
            &OpenOptions {
                limits: Limits {
                    stream_bytes: 16,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            annotation_appearance(&document, &annotation, area)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    for text in [
        b"<< /Subtype /FileAttachment /Name (Graph) >>".as_slice(),
        b"<< /Subtype /FileAttachment /Name 42 >>",
        b"<< /Subtype /FileAttachment /C [0 0] >>",
        b"<< /Subtype /FileAttachment /CA /Opaque >>",
    ] {
        assert_eq!(generate(text, area).unwrap_err().kind, ErrorKind::Syntax);
        assert_eq!(
            generate(text, rect(8.0, 8.0, 8.0, 24.0)).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
    assert_eq!(
        generate(b"<< /Subtype /FileAttachment >>", rect(8.0, 8.0, 8.0, 24.0)).unwrap(),
        None
    );
}

#[test]
fn ink_paths_resolve_indirect_arrays_and_coordinates() {
    let document = Document::open(
        fixture("ink_annotations", "indirect.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let annotation = dictionary(b"<< /Subtype /Ink /InkList 6 0 R /BS << /W 2 >> >>");
    let area = rect(0.0, 0.0, 32.0, 32.0);
    assert_eq!(
        annotation_appearance(&document, &annotation, area).unwrap(),
        generate(
            b"<< /Subtype /Ink /InkList [[4 8 20 8] [20 12 20 24]] /BS << /W 2 >> >>",
            area,
        )
        .unwrap()
    );
}

#[test]
fn ink_validates_geometry_even_when_color_or_width_suppresses_marks() {
    let area = rect(0.0, 0.0, 32.0, 32.0);
    for style in ["/C []", "/BS << /W 0 >>"] {
        assert_eq!(
            generate(
                format!("<< /Subtype /Ink /InkList [[4 8 28 8]] {style} >>").as_bytes(),
                area,
            )
            .unwrap(),
            None
        );
        for value in [
            "null",
            "42",
            "[]",
            "[[]]",
            "[42]",
            "[[4 8 20]]",
            "[[4 /Bad]]",
        ] {
            assert_eq!(
                generate(
                    format!("<< /Subtype /Ink /InkList {value} {style} >>").as_bytes(),
                    area,
                )
                .unwrap_err()
                .kind,
                ErrorKind::Syntax,
                "{value} {style}"
            );
        }
    }
    assert!(
        generate(
            b"<< /Subtype /Ink /InkList [[4 8 28 8]] /ca /NotUsedByStrokes >>",
            area,
        )
        .is_ok()
    );
}

#[test]
fn ink_generation_bounds_aggregate_segments_storage_and_output() {
    for limits in [
        Limits {
            path_segments: 3,
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
            fixture("ink_annotations", "indirect.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        // Repeated indirect paths must charge their aggregate, even though
        // the document cache holds only one source coordinate array.
        let annotation = dictionary(b"<< /Subtype /Ink /InkList [7 0 R 7 0 R] >>");
        assert_eq!(
            annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn polygon_paths_override_vertices_and_resolve_indirect_operands() {
    let area = rect(0.0, 0.0, 32.0, 32.0);
    for subtype in ["Polygon", "PolyLine"] {
        let vertices = format!("<< /Subtype /{subtype} /Vertices [4 4 28 4 4 28] >>");
        let path =
            format!("<< /Subtype /{subtype} /Vertices /Ignored /Path [[4 4] [28 4] [4 28]] >>");
        assert_eq!(
            generate(vertices.as_bytes(), area).unwrap(),
            generate(path.as_bytes(), area).unwrap()
        );
    }
    let document = Document::open(
        fixture("polygon_annotations", "indirect-path.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        annotation_appearance(
            &document,
            &dictionary(b"<< /Subtype /Polygon /Path 6 0 R /IC [1 0 0] /BS << /W 0 >> >>"),
            area,
        )
        .unwrap(),
        generate(
            b"<< /Subtype /Polygon /Path [[4 8] [12 8 20 12 28 20] [28 4] [4 4]] /IC [1 0 0] /BS << /W 0 >> >>",
            area,
        )
        .unwrap()
    );
}

#[test]
fn polygon_geometry_is_bounded_before_empty_mark_selection() {
    for geometry in ["/Vertices [4 4 28 4 4 28]", "/Path [[4 4] [28 4] [4 28]]"] {
        for limits in [
            Limits {
                path_segments: 2,
                ..Limits::default()
            },
            Limits {
                render_bytes: 1,
                ..Limits::default()
            },
        ] {
            let document = Document::open(
                fixture("forms", "text-quadding-0.pdf"),
                &OpenOptions {
                    limits,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let annotation =
                dictionary(format!("<< /Subtype /Polygon {geometry} /BS << /W 0 >> >>").as_bytes());
            assert_eq!(
                annotation_appearance(&document, &annotation, rect(0.0, 0.0, 32.0, 32.0))
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit,
            );
        }
    }
}

#[test]
fn ink_path_ignores_legacy_geometry_and_null_path_restores_it() {
    let area = rect(0.0, 0.0, 32.0, 32.0);
    let expected = generate(b"<< /Subtype /Ink /InkList [[4 8 28 8]] >>", area).unwrap();
    assert_eq!(
        generate(
            b"<< /Subtype /Ink /InkList [[16 4 16 28]] /Path [[4 8] [28 8]] >>",
            area,
        )
        .unwrap(),
        expected
    );
    for inklist in [
        "",
        "/InkList null",
        "/InkList 42",
        "/InkList [[0 /Bad]]",
        "/InkList 999 0 R",
    ] {
        assert_eq!(
            generate(
                format!("<< /Subtype /Ink {inklist} /Path [[4 8] [28 8]] >>").as_bytes(),
                area,
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        generate(
            b"<< /Subtype /Ink /InkList [[4 8 28 8]] /Path null >>",
            area
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        generate(b"<< /Subtype /Ink /InkList 42 /Path null >>", area)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    let document = Document::open(
        fixture("ink_annotations", "pdf2-indirect.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(
        annotation_appearance(
            &document,
            &dictionary(b"<< /Subtype /Ink /InkList [[0 0]] /Path 6 0 R >>"),
            area,
        )
        .unwrap(),
        generate(
            b"<< /Subtype /Ink /InkList [[0 0]] /Path [[4 8] [12 8 20 8 28 8]] >>",
            area
        )
        .unwrap()
    );
}
