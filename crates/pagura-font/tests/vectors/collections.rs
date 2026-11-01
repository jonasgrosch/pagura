//! Full-file collection owners and independently authored selected outlines.
use super::*;
use pagura_font::FontFace;
use std::sync::Arc;

fn source(kind: &str, variant: &str) -> Arc<[u8]> {
    pagura_testkit::fixture("font_collections", &format!("native-{kind}-{variant}.font")).into()
}

fn selected(
    bytes: Arc<[u8]>,
    format: FontFormat,
    face: FontFace<'_>,
    limits: &Limits,
) -> pagura_core::Result<FontProgram> {
    FontProgram::parse_with_face(
        bytes,
        format,
        face,
        limits,
        &mut Diagnostics::new(Recovery::Strict),
    )
}

fn rectangle(font: &FontProgram, index: u32) {
    let (width, height) = if index == 0 { (100, 200) } else { (200, 400) };
    let expected = format!("outline 1\nm 0 0\nl {width} 0\nl {width} {height}\nl 0 {height}\nh\n");
    check_vector(
        font,
        &expected,
        "collection",
        font.format(),
        &mut Diagnostics::new(Recovery::Strict),
    );
}

#[test]
fn indices_and_ordered_names_select_shared_glyf_and_cff_faces() {
    for (kind, format) in [
        ("glyf", FontFormat::TrueType),
        ("cff", FontFormat::OpenType),
    ] {
        for variant in ["v1", "v2", "late-directory", "dsig"] {
            let bytes = source(kind, variant);
            for (index, name) in [
                (0, b"CollectionFirst".as_slice()),
                (1, b"CollectionSecond".as_slice()),
            ] {
                for face in [
                    FontFace::Index(index),
                    FontFace::PostScriptNames(&[b"absent", name]),
                ] {
                    let font = selected(bytes.clone(), format, face, &Limits::default()).unwrap();
                    assert_eq!(font.has_cff_outlines(), kind == "cff");
                    assert_eq!(font.data().as_ptr(), bytes.as_ptr());
                    assert_eq!(font.data(), bytes.as_ref());
                    rectangle(&font, index);
                    assert_eq!(font.advance(GlyphId(1)).unwrap(), Some(600.0));
                }
            }
            let default = FontProgram::parse(bytes.clone(), format, &Limits::default()).unwrap();
            rectangle(&default, 0);
            let ranked = selected(
                bytes,
                format,
                FontFace::PostScriptNames(&[b"CollectionSecond", b"CollectionFirst"]),
                &Limits::default(),
            )
            .unwrap();
            rectangle(&ranked, 1);
        }
    }
}

#[test]
fn standalone_programs_ignore_collection_name_hints() {
    for (kind, format) in [
        ("glyf", FontFormat::TrueType),
        ("cff", FontFormat::OpenType),
    ] {
        let bytes = source(kind, "standalone");
        rectangle(
            &selected(
                bytes.clone(),
                format,
                FontFace::PostScriptNames(&[b"unrelated"]),
                &Limits::default(),
            )
            .unwrap(),
            0,
        );
        assert_eq!(
            selected(bytes, format, FontFace::Index(1), &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArgument
        );
    }
}

#[test]
fn collection_selection_and_input_limits_are_explicit() {
    let bytes = source("glyf", "v1");
    let limits = Limits::default();
    let names = FontFace::PostScriptNames(&[b"CollectionSecond"]);
    for limited in [
        Limits {
            file_bytes: bytes.len() - 1,
            ..limits
        },
        Limits {
            decoded_bytes: bytes.len() - 1,
            ..limits
        },
        Limits {
            font_work: 1,
            ..limits
        },
        Limits {
            font_entries: 1,
            ..limits
        },
        Limits {
            string_bytes: 1,
            ..limits
        },
    ] {
        assert_eq!(
            selected(bytes.clone(), FontFormat::TrueType, names, &limited)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    assert_eq!(
        selected(
            bytes.clone(),
            FontFormat::TrueType,
            FontFace::Index(2),
            &limits
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
    let error = selected(
        bytes,
        FontFormat::TrueType,
        FontFace::PostScriptNames(&[b"missing"]),
        &limits,
    )
    .unwrap_err();
    assert_eq!(
        (error.kind, error.position, error.message),
        (
            ErrorKind::Syntax,
            Position::Buffer(12),
            "font collection name not found"
        )
    );
}

#[test]
fn collection_outline_family_is_independent_of_the_container_hint() {
    for kind in ["glyf", "cff"] {
        let font = selected(
            source(kind, "v1"),
            FontFormat::TrueType,
            FontFace::Index(0),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(font.has_cff_outlines(), kind == "cff");
    }
}
