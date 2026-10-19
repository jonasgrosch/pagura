//! Tile, cache and fallback invariants of the row-fragment path.
use crate::{Accuracy, RenderContext, RenderOptions, Tile};
use pagura_document::{Document, OpenOptions};
use std::sync::Arc;

fn context(group: &str, name: &str) -> RenderContext {
    RenderContext::new(Arc::new(
        Document::open(
            pagura_testkit::fixture(group, name),
            &OpenOptions::default(),
        )
        .unwrap(),
    ))
}

fn options(dpi: f64) -> RenderOptions {
    RenderOptions {
        accuracy: Accuracy::RowFragments,
        ..RenderOptions::from_dpi(dpi)
    }
}

#[test]
fn warm_cold_and_uncached_renders_agree() {
    for name in ["text-cff.pdf", "text-type1.pdf", "text-heavy-page.pdf"] {
        let context = context("paint", name);
        let page = context.document().page(0).unwrap();
        for dpi in [72.0, 150.0, 300.0] {
            let options = options(dpi);
            let cold = context.render_page(page, &options).unwrap();
            assert!(context.retained_bytes() > 0);
            let warm = context.render_page(page, &options).unwrap();
            assert_eq!(cold, warm);
            let list = context.compile_page(page).unwrap();
            assert_eq!(list.render(&options).unwrap(), warm);
        }
    }
}

#[test]
fn any_tile_partition_assembles_into_the_page() {
    for (group, name) in [("paint", "text-heavy-page.pdf"), ("paint", "text-cff.pdf")] {
        let context = context(group, name);
        let page = context.document().page(0).unwrap();
        // One resolution on each side of the lattice change.
        for dpi in [72.0, 150.0] {
            let options = options(dpi);
            let full = context.render_page(page, &options).unwrap();
            let (width, height) = (full.width(), full.height());
            for side in [37_u32, 128] {
                for y in (0..height).step_by(side as usize) {
                    for x in (0..width).step_by(side as usize) {
                        let tile = Tile {
                            x,
                            y,
                            width: (width - x).min(side),
                            height: (height - y).min(side),
                        };
                        let pixels = context.render_tile(page, &options, tile).unwrap();
                        for row in 0..tile.height {
                            let start = ((y + row) * width + x) as usize * 4;
                            let length = tile.width as usize * 4;
                            assert_eq!(
                                pixels.row(row).unwrap(),
                                &full.data()[start..start + length],
                                "{name} {dpi} dpi tile {tile:?} row {row}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_pages_render_exactly() {
    let context = context("content", "blend-Multiply.pdf");
    let page = context.document().page(0).unwrap();
    assert_eq!(
        context.render_page(page, &options(72.0)).unwrap(),
        context
            .render_page(page, &RenderOptions::default())
            .unwrap()
    );
}
