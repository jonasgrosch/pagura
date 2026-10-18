//! Pixel oracles, tile parity and hostile-input admission for text and images.
use crate::{RenderOptions, Tile, compile_page};
use pagura_core::{ErrorKind, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::{fixture, fixture_path};

fn open(name: &str) -> Document {
    Document::open(
        fixture("paint", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
fn independent_paint_oracles_and_device_tiles() {
    let directory = fixture_path("paint", "");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ppm"))
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let oracle = std::fs::read(&path).unwrap();
        let (width, height) = list.device_size(&RenderOptions::default()).unwrap();
        let header = format!("P6\n{width} {height}\n255\n");
        let data = oracle.strip_prefix(header.as_bytes()).unwrap();
        let image = list.render(&RenderOptions::default()).unwrap();
        let tolerance = u8::from(
            matches!(
                name,
                "text-triangle-area" | "image-dct" | "text-knockout-opaque-overlap"
            ) || name.starts_with("image-area-"),
        );
        for (actual, expected) in image
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(data.as_chunks::<3>().0)
        {
            for i in 0..3 {
                assert!(
                    actual[i].abs_diff(expected[i]) <= tolerance,
                    "{name}: {actual:?} != {expected:?}"
                );
            }
            assert_eq!(actual[3], 255);
        }
        for scale in [1.0, 1.5] {
            let options = RenderOptions {
                scale,
                ..Default::default()
            };
            let full = list.render(&options).unwrap();
            let (width, height) = list.device_size(&options).unwrap();
            for y in (0..height).step_by(if height > 64 { 37 } else { 7 }) {
                for x in (0..width).step_by(if width > 64 { 41 } else { 9 }) {
                    let tile = Tile {
                        x,
                        y,
                        width: (width - x).min(if width > 64 { 41 } else { 9 }),
                        height: (height - y).min(if height > 64 { 37 } else { 7 }),
                    };
                    let image = list.render_tile(&options, tile).unwrap();
                    for row in 0..tile.height {
                        let offset = usize::try_from(x).unwrap() * 4;
                        let count = usize::try_from(tile.width).unwrap() * 4;
                        assert_eq!(
                            image.row(row).unwrap(),
                            &full.row(y + row).unwrap()[offset..offset + count],
                            "{name} scale {scale}: {tile:?}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn paint_cache_admission_is_atomic_and_shared_across_saved_states() {
    let document = open("text-mode-0");
    let mut compiler = super::super::Compiler::new(&document, document.page(0).unwrap()).unwrap();
    compiler.paint_charge(10, 1).unwrap();
    compiler.save_graphics().unwrap();
    compiler.paint_charge(20, 1).unwrap();
    compiler.restore_graphics().unwrap();
    assert_eq!(compiler.state.text.cache.borrow().bytes, 30);
    let error = compiler.paint_charge(usize::MAX, 0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(compiler.state.text.cache.borrow().bytes, 30);
    assert_eq!(compiler.state.text.cache.borrow().entries, 2);
}
#[test]
fn fonts_and_outlines_are_reused() {
    let document = open("text-tj-adjustments");
    let mut compiler = super::super::Compiler::new(&document, document.page(0).unwrap()).unwrap();
    compiler.page(document.page(0).unwrap()).unwrap();
    let cache = compiler.state.text.cache.borrow();
    assert_eq!(cache.font_counts(), (1, 1));
}

#[test]
fn malformed_font_programs_report_their_own_stream() {
    let mut bytes = fixture("paint", "text-mode-0.pdf");
    let program_start = bytes
        .windows(11)
        .position(|b| b == b"stream\n\0\x01\0\0")
        .unwrap()
        + 7;
    // An unknown sfnt version over intact tables is a reported repair: strict
    // mode rejects it with the tag, and both modes locate it in the font
    // stream (object 7), not in the content stream that selected the font.
    bytes[program_start..program_start + 4].copy_from_slice(b"bad!");
    let font_stream = pagura_core::Position::Stream {
        object: pagura_core::ObjectId::new(7, 0),
        offset: 0,
    };
    let strict = Document::open(
        bytes.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(
        error.repair.map(pagura_core::Repair::as_str),
        Some("font.sfnt-version")
    );
    assert_eq!(error.position, font_stream);
    assert_eq!(error.object, Some(pagura_core::ObjectId::new(7, 0)));
    let repaired = Document::open(bytes, &OpenOptions::default()).unwrap();
    compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
    let diagnostic = repaired
        .diagnostics()
        .into_iter()
        .find(|item| item.repair.as_str() == "font.sfnt-version")
        .unwrap();
    assert_eq!(diagnostic.position, font_stream);
    assert_eq!(diagnostic.object, Some(pagura_core::ObjectId::new(7, 0)));
}

#[test]
fn text_clip_resets_report_positions_and_preserve_accumulated_outlines() {
    for (name, offsets) in [
        ("text-clip-reset", &[10][..]),
        ("text-clip-reset-retained", &[42][..]),
        ("text-clip-reset-reentered", &[33, 57][..]),
        ("text-clip-reset-restored", &[33][..]),
        ("text-clip-reset-inherited", &[33][..]),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        let object = pagura_core::ObjectId::new(4, 0);
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.repair.unwrap().as_str(), "text.clip-mode-reset");
        assert_eq!(error.object, Some(object));
        assert_eq!(
            error.position,
            pagura_core::Position::Stream {
                object,
                offset: offsets[0],
            }
        );
        assert_eq!(document.diagnostics(), []);
        let repaired = Document::open(
            fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let page = repaired.page(0).unwrap();
        let list = compile_page(&repaired, page).unwrap();
        let image = list.render(&RenderOptions::default()).unwrap();
        assert_eq!(
            pagura_testkit::ppm(
                image.width(),
                image.height(),
                &image.to_rgb_over(pagura_raster::Rgba8::WHITE)
            ),
            fixture("paint", &format!("{name}.rgb")),
            "{name}"
        );
        let diagnostics = repaired.diagnostics();
        assert_eq!(diagnostics.len(), offsets.len());
        for (diagnostic, &offset) in diagnostics.iter().zip(offsets) {
            assert_eq!(diagnostic.repair.as_str(), "text.clip-mode-reset");
            assert_eq!(diagnostic.object, Some(object));
            assert_eq!(
                diagnostic.position,
                pagura_core::Position::Stream { object, offset }
            );
        }
        compile_page(&repaired, page).unwrap();
        assert_eq!(repaired.diagnostics(), diagnostics);
    }
}
