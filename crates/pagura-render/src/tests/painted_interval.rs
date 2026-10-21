//! Independent integer-edge page, tile and recycled-output expectations.

use crate::{Accuracy, Pixmap, RenderContext, RenderOptions, Tile, compile_page};
use pagura_core::Recovery;
use pagura_document::{Document, OpenOptions};
use pagura_raster::Rgba8;
use std::sync::Arc;

const SIDE: u32 = 769;
const BACKGROUND: Rgba8 = Rgba8::opaque(17, 85, 170);

fn document(alpha: &str) -> Document {
    // Page 0 has black integer rectangles; page 1 is empty. Literal
    // PDF coordinates are independent of the top-down occupancy oracle below.
    let content = "/Paint gs 0 g 254 509 6 6 re f 510 255 4 4 re f \
                   0 768 1 1 re f 0 512 1 1 re f \
                   255 257 1 1 re f 768 0 1 1 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {SIDE} {SIDE}] \
             /Resources << /ExtGState << /Paint << /ca {alpha} >> >> >> /Contents 5 0 R >>"
        ),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {SIDE} {SIDE}] \
             /Resources << >> /Contents 6 0 R >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
    ];
    // Classic-xref construction follows tests::jpeg_page; no renderer output
    // or existing baseline pixels are used to construct expected bytes.
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    Document::open(
        pdf,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

fn expected(tile: Tile, painted: bool, background: Rgba8, foreground: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in tile.y..tile.y + tile.height {
        for x in tile.x..tile.x + tile.width {
            // At scale 1, PDF [x,y,w,h] occupies device rows
            // SIDE-y-h .. SIDE-y. All edges here are integral, hence no
            // coverage rounding or engine-specific tolerance is involved.
            let black = painted
                && (((254..260).contains(&x) && (254..260).contains(&y))
                    || ((510..514).contains(&x) && (510..514).contains(&y))
                    || [(0, 0), (0, 256), (255, 511), (768, 768)].contains(&(x, y)));
            let pixel = if black {
                foreground
            } else {
                [background.r, background.g, background.b, background.a]
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    pixels
}

fn assert_pixels(
    image: &Pixmap,
    tile: Tile,
    painted: bool,
    background: Rgba8,
    foreground: [u8; 4],
) {
    assert_eq!((image.width(), image.height()), (tile.width, tile.height));
    pagura_testkit::assert_bytes_eq(
        image.data(),
        &expected(tile, painted, background, foreground),
        "independent integer-rectangle RGBA",
    );
}

fn whole() -> Tile {
    Tile {
        x: 0,
        y: 0,
        width: SIDE,
        height: SIDE,
    }
}

#[test]
fn integer_rectangles_match_irregular_and_offset_tiles_in_both_modes() {
    // Black at alpha 1/2 leaves [17,85,170]/2, rounded once to [9,43,85].
    for (alpha, foreground) in [("1", [0, 0, 0, 255]), (".5", [9, 43, 85, 255])] {
        check_tiles(alpha, foreground);
    }
}

fn check_tiles(alpha: &str, foreground: [u8; 4]) {
    let document = document(alpha);
    let list = compile_page(&document, document.page(0).unwrap()).unwrap();
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        let options = RenderOptions {
            accuracy,
            scale: 1.0,
            background: BACKGROUND,
            ..RenderOptions::default()
        };
        let full = list.render(&options).unwrap();
        assert_pixels(&full, whole(), true, BACKGROUND, foreground);
        // One complete partition has widths AND heights 1, 255, 256, 257.
        // Full rendering crosses both x/y=256 and 512; the final one-pixel
        // chunk at 768 and empty chunks also have independent expectations.
        let edges = [0, 1, 256, 512, SIDE];
        let mut assembled = vec![231; (SIDE * SIDE * 4) as usize];
        for ys in edges.windows(2) {
            for xs in edges.windows(2) {
                let tile = Tile {
                    x: xs[0],
                    y: ys[0],
                    width: xs[1] - xs[0],
                    height: ys[1] - ys[0],
                };
                let part = list.render_tile(&options, tile).unwrap();
                assert_pixels(&part, tile, true, BACKGROUND, foreground);
                for row in 0..tile.height {
                    let source = (row * tile.width * 4) as usize;
                    let target = (((tile.y + row) * SIDE + tile.x) * 4) as usize;
                    let count = (tile.width * 4) as usize;
                    assembled[target..target + count]
                        .copy_from_slice(&part.data()[source..source + count]);
                }
            }
        }
        pagura_testkit::assert_bytes_eq(
            &assembled,
            &expected(whole(), true, BACKGROUND, foreground),
            "partition",
        );
        pagura_testkit::assert_bytes_eq(&assembled, full.data(), "full versus partition");

        for (x, y, width, height) in [
            (253, 253, 7, 7),
            (255, 255, 257, 257),
            (256, 256, 257, 257),
            (509, 509, 7, 7),
            (0, 256, 256, 256),
            (600, 600, 64, 64),
            (768, 768, 1, 1),
        ] {
            let tile = Tile {
                x,
                y,
                width,
                height,
            };
            let part = list.render_tile(&options, tile).unwrap();
            assert_pixels(&part, tile, true, BACKGROUND, foreground);
        }
    }
    assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
}

#[test]
fn context_painted_then_blank_output_refills_recycled_storage_in_both_modes() {
    for (alpha, foreground) in [("1", [0, 0, 0, 255]), (".5", [9, 43, 85, 255])] {
        check_recycled_output(alpha, foreground);
    }
}

fn check_recycled_output(alpha: &str, foreground: [u8; 4]) {
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        let context = RenderContext::new(Arc::new(document(alpha)));
        let options = RenderOptions {
            accuracy,
            scale: 1.0,
            background: BACKGROUND,
            ..RenderOptions::default()
        };
        let painted = context
            .render_page(context.document().page(0).unwrap(), &options)
            .unwrap();
        assert_pixels(&painted, whole(), true, BACKGROUND, foreground);
        let address = painted.data().as_ptr();
        context.recycle(painted);
        // A different opaque backdrop catches both stale marks and an
        // incorrectly retained initialization colour in skipped pixels.
        let blank_options = RenderOptions {
            background: Rgba8::opaque(211, 37, 93),
            ..options
        };
        let blank = context
            .render_page(context.document().page(1).unwrap(), &blank_options)
            .unwrap();
        assert_pixels(&blank, whole(), false, blank_options.background, foreground);
        // Only Competitive borrows the public context's spare surface today.
        // Prove that path really reused storage rather than merely comparing
        // two fresh allocations. Exact still receives the same pixel checks.
        if accuracy == Accuracy::Competitive {
            assert_eq!(blank.data().as_ptr(), address);
        }
        context.recycle(blank);
        let repainted = context
            .render_page(context.document().page(0).unwrap(), &options)
            .unwrap();
        assert_pixels(&repainted, whole(), true, BACKGROUND, foreground);
        if accuracy == Accuracy::Competitive {
            assert_eq!(repainted.data().as_ptr(), address);
        }
        assert_eq!(
            context.document().diagnostics(),
            [] as [pagura_core::Diagnostic; 0]
        );
    }
}
