#![forbid(unsafe_code)]
//! A context records each page once, keeps the scene in its store and
//! renders it by region; free functions and contexts agree.

use pagura_document::{Document, OpenOptions};
use pagura_raster::Rgba8;
use pagura_render::{RenderContext, RenderOptions, Tile};
use pagura_testkit::{assert_bytes_eq, fixture, open_fixture, ppm};
use std::sync::Arc;

fn open(area: &str, name: &str) -> Arc<Document> {
    Arc::new(open_fixture(area, name, |bytes| {
        Document::open(bytes, &OpenOptions::default())
    }))
}

fn rgb(pixmap: &pagura_render::Pixmap) -> Vec<u8> {
    let data = pixmap.to_rgb_over(Rgba8::WHITE);
    ppm(pixmap.width(), pixmap.height(), &data)
}

#[test]
fn scenes_are_recorded_once_and_shared_through_the_store() {
    let document = open("classic", "rectangle.pdf");
    let context = RenderContext::new(Arc::clone(&document));
    let page = document.page(0).unwrap();
    let first = context.scene(page).unwrap();
    let retained = context.retained_bytes();
    assert!(retained > 0, "the scene is retained");
    let again = context.scene(page).unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(context.retained_bytes(), retained);
    let uncached = RenderContext::with_store_capacity(document, None, 0);
    let page = uncached.document().page(0).unwrap();
    let a = uncached.scene(page).unwrap();
    let b = uncached.scene(page).unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(uncached.retained_bytes(), 0);
}

#[test]
fn pages_match_their_oracles_and_tiles_assemble_into_them() {
    let document = open("classic", "rectangle.pdf");
    let context = RenderContext::new(Arc::clone(&document));
    let page = document.page(0).unwrap();
    let options = RenderOptions::default();
    let whole = context.render_page(page, &options).unwrap();
    assert_bytes_eq(
        &rgb(&whole),
        &fixture("classic", "rectangle.ppm"),
        "rectangle",
    );
    let free = pagura_render::render_page(&document, page, &options, None).unwrap();
    assert_eq!(free.data(), whole.data());
    let tile = Tile {
        x: 5,
        y: 7,
        width: 30,
        height: 20,
    };
    let part = context.render_tile(page, &options, tile).unwrap();
    for row in 0..tile.height {
        let start = usize::try_from(tile.x * 4).unwrap();
        let end = start + usize::try_from(tile.width * 4).unwrap();
        assert_eq!(
            part.row(row).unwrap(),
            &whole.row(tile.y + row).unwrap()[start..end],
            "row {row}"
        );
    }
}

#[test]
fn text_comes_from_the_text_device_alone() {
    let document = open("text", "extract-simple.pdf");
    let context = RenderContext::new(Arc::clone(&document));
    let page = document.page(0).unwrap();
    let text = context.text_page(page).unwrap();
    assert_eq!(text.text(), "ZZ");
    let free = pagura_render::text_page(&document, page, None).unwrap();
    assert_eq!(free.text(), text.text());
}
