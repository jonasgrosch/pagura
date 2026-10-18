//! Pixel oracles and tile parity for text and images.

use pagura_core::Recovery;
use pagura_document::{Document, OpenOptions};
use pagura_render::{RenderOptions, Tile, compile_page};
use pagura_testkit::{fixture, fixture_path};

#[expect(
    clippy::unwrap_used,
    reason = "a fixture that fails to open fails the test"
)]
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
    let manifest = pagura_testkit::Manifest::load("paint");
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let document = open(name);
        let list = if name.starts_with("image-fax-grid-") || name == "image-inline-fax-grid" {
            let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
            assert_eq!(
                error.repair,
                Some(pagura_image::tags::CCITT_COLUMNS),
                "{name}"
            );
            assert!(document.diagnostics().is_empty(), "{name}");
            let repaired = Document::open(
                fixture("paint", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            let list = compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
            assert_eq!(
                repaired
                    .diagnostics()
                    .iter()
                    .filter_map(pagura_core::Diagnostic::repair)
                    .collect::<Vec<_>>(),
                [pagura_image::tags::CCITT_COLUMNS],
                "{name}"
            );
            list
        } else {
            compile_page(&document, document.page(0).unwrap()).unwrap()
        };
        let oracle = std::fs::read(&path).unwrap();
        let (width, height) = list.device_size(&RenderOptions::default()).unwrap();
        let header = format!("P6\n{width} {height}\n255\n");
        let data = oracle.strip_prefix(header.as_bytes()).unwrap();
        let image = list.render(&RenderOptions::default()).unwrap();
        let tolerance = super::support::tolerance(&manifest, name);
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
            let options = RenderOptions::default().with_scale(scale);
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
