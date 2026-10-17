//! Independent generated-vector and API invariant tests.
use crate::{CmapSubtable, FontFormat, FontProgram, GlyphId};
use pagura_core::{ErrorKind, Limits, Position};
use pagura_raster::{PathBuilder, Point, Segment};

fn font(name: &str, format: FontFormat) -> FontProgram {
    FontProgram::parse(
        pagura_testkit::fixture("font", &format!("{name}.font")),
        format,
        &Limits::default(),
    )
    .unwrap()
}
fn vector(name: &str, format: FontFormat) {
    let bytes = pagura_testkit::fixture("font", &format!("{name}.font"));
    let expected =
        String::from_utf8(pagura_testkit::fixture("font", &format!("{name}.expect"))).unwrap();
    let Some(font) = expected_parse(bytes, format, &expected, name) else {
        return;
    };
    assert_eq!(font.format(), format);
    let mut glyph = None;
    let mut builder = PathBuilder::new();
    for line in expected.lines() {
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        let n = |i: usize| words[i].parse::<f64>().unwrap();
        let u = |i: usize| words[i].parse::<u32>().unwrap();
        match words[0] {
            "count" => assert_eq!(font.glyph_count(), u(1), "{name}"),
            "matrix" => check_matrix(&font, &[n(1), n(2), n(3), n(4), n(5), n(6)]),
            "name" => assert_eq!(
                font.glyph_for_name(words[1].as_bytes()),
                Some(GlyphId(u(2))),
                "{name}"
            ),
            "advance" => assert_eq!(font.advance(GlyphId(u(1))), Some(n(2)), "{name}"),
            "is-cid" => assert_eq!(font.is_cid_keyed(), u(1) == 1),
            "cid" => assert_eq!(font.glyph_for_cid(u(1)), Some(GlyphId(u(2)))),
            "encoding" => assert_eq!(
                font.builtin_encoding().unwrap()[usize::try_from(u(1)).unwrap()]
                    .as_ref()
                    .unwrap()
                    .as_bytes(),
                words[2].as_bytes()
            ),
            "has" => {
                let selection = selection(words[1]);
                assert_eq!(font.has_cmap_subtable(selection), u(2) != 0, "{name}");
            }
            "absent" => assert_eq!(
                font.glyph_for_code(selection(words[1]), u(2)),
                None,
                "{name}"
            ),
            "code" => {
                let selection = selection(words[1]);
                assert_eq!(
                    font.glyph_for_code(selection, u(2)),
                    Some(GlyphId(u(3))),
                    "{name}"
                );
            }
            "budget" => budget(
                &font,
                GlyphId(u(1)),
                usize::try_from(u(2)).unwrap(),
                usize::try_from(u(3)).unwrap(),
            ),
            "budget-error" => {
                let limited = FontProgram::parse(
                    font.data().to_vec(),
                    format,
                    &Limits {
                        operations: usize::try_from(u(2)).unwrap(),
                        ..Limits::default()
                    },
                )
                .unwrap();
                let error = limited.outline(GlyphId(u(1))).unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax);
                assert!(matches!(error.position, Position::Buffer(_)));
            }
            "outline" => {
                check_outline(&font, name, glyph.take(), &mut builder);
                glyph = Some(GlyphId(u(1)));
            }
            "outline-error" => outline_error(&font, GlyphId(u(1)), name),
            "m" => {
                builder.move_to(Point::new(n(1), n(2)));
            }
            "l" => {
                builder.line_to(Point::new(n(1), n(2))).unwrap();
            }
            "c" => {
                builder
                    .cubic_to(
                        Point::new(n(1), n(2)),
                        Point::new(n(3), n(4)),
                        Point::new(n(5), n(6)),
                    )
                    .unwrap();
            }
            "h" => {
                builder.close();
            }
            _ => panic!("invalid font expectation"),
        }
    }
    check_outline(&font, name, glyph, &mut builder);
    assert!(font.outline(GlyphId(u32::MAX)).is_err());
    assert_eq!(font.advance(GlyphId(u32::MAX)), None);
    assert_eq!(font.glyph_for_name(b"no such glyph"), None);
}

#[test]
fn independent_truetype_outlines_metrics_and_cmaps() {
    for name in [
        "truetype-cmap0",
        "truetype-cmap4",
        "truetype-cmap6",
        "truetype-cmap12",
        "truetype-short-flags",
        "truetype-repeat-flags",
        "truetype-implied-points",
    ] {
        vector(name, FontFormat::TrueType);
    }
}
#[test]
fn independent_cff_outlines_cid_selection_and_encodings() {
    for name in [
        "cff",
        "cff-cid",
        "cff-hints",
        "cff-global",
        "cff-flex",
        "cff-arithmetic",
        "cff-cid-fd-array",
    ] {
        vector(name, FontFormat::Cff);
    }
    for name in ["opentype-cff", "cff2", "cff2-default-blend"] {
        vector(name, FontFormat::OpenType);
    }
}
#[test]
fn invalid_tables_charstrings_and_cycles_have_buffer_positions() {
    for name in [
        "truncated-directory",
        "recursive-component",
        "truncated-glyph",
        "oversized-directory",
        "invalid-cmap-range",
        "descending-loca",
        "truncated-hmtx",
        "invalid-post",
    ] {
        vector(name, FontFormat::TrueType);
    }
    for name in [
        "cff-recursive",
        "cff-stack",
        "cff-mask",
        "cff-seac-cycle",
        "cff-index-overflow",
        "cff-nonfinite",
    ] {
        vector(name, FontFormat::Cff);
    }
}
#[test]
fn budgets_and_blank_glyphs() {
    for (name, format) in [
        ("cff", FontFormat::Cff),
        ("truetype-cmap4", FontFormat::TrueType),
    ] {
        let parsed = font(name, format);
        assert!(parsed.outline(GlyphId(0)).unwrap().is_empty());
        let bytes = parsed.data().to_vec();
        let limits = Limits {
            file_bytes: bytes.len() - 1,
            ..Limits::default()
        };
        assert_eq!(
            FontProgram::parse(bytes.clone(), format, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        for limits in [
            Limits {
                operations: 1,
                ..Limits::default()
            },
            Limits {
                path_segments: 1,
                ..Limits::default()
            },
            Limits {
                depth: 0,
                ..Limits::default()
            },
            Limits {
                value_nodes: 1,
                ..Limits::default()
            },
        ] {
            match FontProgram::parse(bytes.clone(), format, &limits) {
                Ok(p) => assert!(
                    p.outline(GlyphId(if format == FontFormat::TrueType { 3 } else { 1 }))
                        .is_err()
                ),
                Err(e) => assert_eq!(e.kind, ErrorKind::Syntax),
            }
        }
    }
}
#[test]
fn cmap_missing_glyph_and_unicode_ranges() {
    let bmp = font("truetype-cmap4", FontFormat::TrueType);
    assert_eq!(
        bmp.glyph_for_code(CmapSubtable::WindowsUnicode, 66),
        Some(GlyphId(0))
    );
    assert_eq!(
        bmp.glyph_for_code(CmapSubtable::WindowsUnicode, 64),
        Some(GlyphId(0))
    );
    assert_eq!(
        bmp.glyph_for_code(CmapSubtable::WindowsUnicode, 0x1f600),
        None
    );
    let full = font("truetype-cmap12", FontFormat::TrueType);
    assert_eq!(
        full.glyph_for_code(CmapSubtable::WindowsUnicodeFull, 0x1f601),
        Some(GlyphId(2))
    );
    assert_eq!(
        full.glyph_for_code(CmapSubtable::WindowsUnicodeFull, 0x1f602),
        None
    );
}
#[test]
fn cmap_subtable_presence_follows_platform_and_encoding() {
    let all = [
        CmapSubtable::MacRoman,
        CmapSubtable::WindowsSymbol,
        CmapSubtable::WindowsUnicode,
    ];
    for (name, present) in [
        ("truetype-cmap0", CmapSubtable::MacRoman),
        ("truetype-cmap6", CmapSubtable::WindowsSymbol),
        ("truetype-cmap4", CmapSubtable::WindowsUnicode),
    ] {
        let program = font(name, FontFormat::TrueType);
        for subtable in all {
            assert_eq!(
                program.has_cmap_subtable(subtable),
                subtable == present,
                "{name} {subtable:?}"
            );
        }
    }
    let cff = font("cff", FontFormat::Cff);
    assert!(all.iter().all(|&subtable| !cff.has_cmap_subtable(subtable)));
}
#[test]
fn truncated_prefixes_never_panic() {
    for (name, format) in [
        ("cff", FontFormat::Cff),
        ("cff-cid", FontFormat::Cff),
        ("truetype-cmap4", FontFormat::TrueType),
        ("cff2", FontFormat::OpenType),
        ("type1-pfa", FontFormat::Type1),
        ("type1-pfb", FontFormat::Type1),
    ] {
        let bytes = pagura_testkit::fixture("font", &format!("{name}.font"));
        for length in 0..bytes.len() {
            if let Ok(p) = FontProgram::parse(bytes[..length].to_vec(), format, &Limits::default())
            {
                for glyph in 0..p.glyph_count() {
                    let _ = p.outline(GlyphId(glyph));
                }
            }
        }
    }
}

#[test]
fn independent_type1_cipher_outlines_flex_and_seac() {
    for name in [
        "type1-pfa",
        "type1-pfb",
        "type1-hex",
        "type1-leniv",
        "type1-recursive",
        "type1-stack",
        "type1-truncated-pfb",
        "type1-hint-replacement",
        "type1-custom-encoding",
        "type1-lexical",
    ] {
        vector(name, FontFormat::Type1);
    }
}

fn check_matrix(font: &FontProgram, expected: &[f64; 6]) {
    let m = font.font_matrix();
    assert_eq!(&[m.a, m.b, m.c, m.d, m.e, m.f], expected);
}

fn compare_segment(a: Segment, b: Segment, name: &str) {
    let point = |a: Point, b: Point| {
        assert!(
            (a.x - b.x).abs() < 1e-10 && (a.y - b.y).abs() < 1e-10,
            "{name}: {a:?} vs {b:?}"
        );
    };
    match (a, b) {
        (Segment::MoveTo(a), Segment::MoveTo(b)) | (Segment::LineTo(a), Segment::LineTo(b)) => {
            point(a, b);
        }
        (Segment::CubicTo(a, b, c), Segment::CubicTo(d, e, f)) => {
            point(a, d);
            point(b, e);
            point(c, f);
        }
        (Segment::Close, Segment::Close) => {}
        _ => panic!("{name}: segment kinds differ"),
    }
}

#[test]
fn mutated_programs_respect_small_budgets() {
    let limits = Limits {
        operations: 2000,
        value_nodes: 1000,
        objects: 1000,
        depth: 8,
        path_segments: 500,
        decoded_bytes: 16384,
        ..Limits::default()
    };
    for (name, format) in [
        ("cff", FontFormat::Cff),
        ("cff-cid-fd-array", FontFormat::Cff),
        ("cff-expert-charset", FontFormat::Cff),
        ("cff2-private-blend", FontFormat::OpenType),
        ("cff2-fdselect4", FontFormat::OpenType),
        ("truetype-cmap2", FontFormat::TrueType),
        ("truetype-cmap10", FontFormat::TrueType),
        ("truetype-cmap13", FontFormat::TrueType),
        ("cff2-default-blend", FontFormat::OpenType),
        ("truetype-cmap4", FontFormat::TrueType),
        ("type1-pfb", FontFormat::Type1),
    ] {
        let bytes = pagura_testkit::fixture("font", &format!("{name}.font"));
        for index in (0..bytes.len()).step_by(3) {
            for byte in [0, 255] {
                let mut damaged = bytes.clone();
                damaged[index] = byte;
                if let Ok(font) = FontProgram::parse(damaged, format, &limits) {
                    for glyph in 0..font.glyph_count().min(6) {
                        let _ = font.outline(GlyphId(glyph));
                        let _ = font.advance(GlyphId(glyph));
                    }
                }
            }
        }
    }
}

#[test]
fn pdf_program_constraints_from_iso_text() {
    for name in [
        "cmap-platform-identities",
        "vertical-tables-ignored",
        "required-glyf-missing",
        "required-head-missing",
        "required-hhea-missing",
        "required-hmtx-missing",
        "required-loca-missing",
        "required-maxp-missing",
    ] {
        vector(name, FontFormat::TrueType);
    }
    for name in ["opentype-cff-pdf-minimum", "opentype-cff-missing-cmap"] {
        vector(name, FontFormat::OpenType);
    }
    let full = font("truetype-cmap12", FontFormat::TrueType);
    assert!(!full.has_cmap_subtable(CmapSubtable::WindowsUnicode));
    assert!(full.has_cmap_subtable(CmapSubtable::WindowsUnicodeFull));
}

#[test]
fn pdf_font_count_and_notdef_rules() {
    vector("cff-multiple-fonts", FontFormat::Cff);
    vector("type1-missing-notdef", FontFormat::Type1);
    vector("type1-paint-type-ignored", FontFormat::Type1);
    vector("truetype-cid-no-cmap", FontFormat::TrueType);
}

fn selection(name: &str) -> CmapSubtable {
    match name {
        "mac" => CmapSubtable::MacRoman,
        "symbol" => CmapSubtable::WindowsSymbol,
        "full" => CmapSubtable::WindowsUnicodeFull,
        "platform0" => CmapSubtable::Unicode,
        _ => CmapSubtable::WindowsUnicode,
    }
}

#[test]
fn extended_font_formats_from_independent_vectors() {
    for name in [
        "cff-standard-sids",
        "cff-expert-charset",
        "cff-expert-subset",
        "cff-custom-charset0",
        "cff-custom-charset1",
        "cff-custom-charset2",
        "cff-custom-encoding0-supplement",
    ] {
        vector(name, FontFormat::Cff);
    }
    for name in [
        "truetype-cmap2",
        "truetype-cmap10",
        "truetype-cmap13",
        "truetype-cmap14",
        "cmap2-invalid-key",
        "cmap2-invalid-offset",
        "cmap10-invalid-count",
        "cmap13-invalid-overlap",
    ] {
        vector(name, FontFormat::TrueType);
    }
    for name in [
        "cff2-fdselect4",
        "cff2-fdselect4-wide",
        "cff2-private-blend",
        "cff2-private-blend-override",
        "cff2-font-dict-matrix",
        "cff2-fd-index",
        "cff2-fd-sentinel",
        "cff2-fd-order",
        "cff2-dict-underflow",
        "cff2-dict-vsindex",
        "cff2-dict-stack",
    ] {
        vector(name, FontFormat::OpenType);
    }
}

fn budget(font: &FontProgram, glyph: GlyphId, operations: usize, segments: usize) {
    let limits = Limits {
        operations,
        path_segments: segments,
        ..Limits::default()
    };
    let p = FontProgram::parse(font.data().to_vec(), font.format(), &limits).unwrap();
    assert_eq!(p.outline(glyph).unwrap().segments().len(), segments);
    for limits in [
        Limits {
            operations: operations - 1,
            ..limits
        },
        Limits {
            path_segments: segments - 1,
            ..limits
        },
    ] {
        let p = FontProgram::parse(font.data().to_vec(), font.format(), &limits).unwrap();
        let error = p.outline(glyph).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert!(matches!(error.position, Position::Buffer(_)));
    }
}
#[test]
fn outline_work_scales_with_input_and_expansion() {
    for n in [64, 128, 256] {
        vector(&format!("cff-linear-{n}"), FontFormat::Cff);
        vector(&format!("cff-linear-subr-{n}"), FontFormat::Cff);
    }
    vector("cff-branching-subrs", FontFormat::Cff);
}

fn check_outline(
    font: &FontProgram,
    name: &str,
    glyph: Option<GlyphId>,
    builder: &mut PathBuilder,
) {
    if let Some(glyph) = glyph {
        let actual = font
            .outline(glyph)
            .unwrap_or_else(|e| panic!("{name} {glyph:?}: {e}"));
        let expected = builder.finish();
        assert_eq!(
            actual.segments().len(),
            expected.segments().len(),
            "{name} {glyph:?}: {actual:?} vs {expected:?}"
        );
        for (a, b) in actual.segments().iter().zip(expected.segments()) {
            compare_segment(*a, *b, name);
        }
    }
}

#[test]
fn compressed_unicode_ranges_do_not_expand() {
    let bytes = pagura_testkit::fixture("font", "truetype-cmap13.font");
    let p = FontProgram::parse(
        bytes,
        FontFormat::TrueType,
        &Limits {
            objects: 8,
            value_nodes: 8,
            operations: 64,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        p.glyph_for_code(CmapSubtable::Unicode, 0x0010_ffff),
        Some(GlyphId(2))
    );
}

fn outline_error(font: &FontProgram, glyph: GlyphId, name: &str) {
    let error = font.outline(glyph).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
    assert!(matches!(error.position, Position::Buffer(_)));
}

fn expected_parse(
    bytes: Vec<u8>,
    format: FontFormat,
    expected: &str,
    name: &str,
) -> Option<FontProgram> {
    let parsed = FontProgram::parse(bytes, format, &Limits::default());
    if let Some(tag) = expected.trim().strip_prefix("unsupported ") {
        let error = parsed.unwrap_err();
        assert_eq!(error.feature().unwrap().as_str(), tag);
        assert!(matches!(error.position, Position::Buffer(_)));
        return None;
    }
    if expected == "parse-error\n" {
        let error = parsed.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(matches!(error.position, Position::Buffer(_)));
        return None;
    }
    Some(parsed.unwrap_or_else(|e| panic!("{name}: {e}")))
}
