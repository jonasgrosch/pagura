//! Tile and cache invariants of row-fragment glyph masks.
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
        accuracy: Accuracy::Competitive,
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
fn ceiling_by_truncation_matches_ceil() {
    let mut values = vec![
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MIN,
        -0.0,
        f32::MIN_POSITIVE,
        16_777_217.0,
        4_294_967_296.0,
        18_446_744_073_709_551_616.0,
    ];
    let mut value = -64.0_f32;
    while value <= 70_000.0 {
        values.extend([value, value.next_up(), value.next_down(), value + 0.5]);
        value += 1.0;
    }
    values.extend((0..=u32::MAX).step_by(4099).map(f32::from_bits));
    for value in values {
        let expected = value.ceil();
        let index = super::glyphs::ceil_by_truncation(value);
        if expected.is_nan() || expected <= 0.0 {
            assert_eq!(index, 0, "{value}");
        } else if expected >= usize::MAX as f32 {
            assert_eq!(index, usize::MAX, "{value}");
        } else {
            assert_eq!(index as f32, expected, "{value}");
        }
    }
}

#[test]
#[expect(clippy::manual_midpoint, reason = "the halved sum is under test")]
fn one_cell_midpoints_match_the_exact_midpoint() {
    // The one-cell path's endpoints: left at or above zero within a raster
    // row, right within the same cell.
    let mut lefts = vec![0.0_f32, f32::from_bits(1), f32::MIN_POSITIVE, 1e-30];
    let mut value = 0.0_f32;
    while value < 4096.0 {
        lefts.extend([
            value,
            value.next_up(),
            value + 0.25,
            value + 0.5,
            (value + 1.0).next_down(),
        ]);
        value += 1.0;
    }
    for left in lefts {
        let next = left.floor() + 1.0;
        for right in [
            left,
            left.next_up(),
            (left + next) * 0.5,
            next.next_down(),
            next,
        ] {
            if right < left || right > next {
                continue;
            }
            assert_eq!(
                ((left + right) * 0.5).to_bits(),
                left.midpoint(right).to_bits(),
                "{left} {right}"
            );
        }
    }
}
