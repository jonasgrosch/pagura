//! Strict/repaired boundaries and tile invariance for recovery and transfers.
use crate::{RenderOptions, Tile, compile_page};
use pagura_core::{ErrorKind, ObjectId, Position, Recovery};
use pagura_document::{Document, ObjectSource, OpenOptions};
use pagura_testkit::fixture;

#[test]
fn coloured_patterns_and_soft_mask_artwork_are_strictly_valid() {
    for name in [
        "pattern-coloured-base-no-operands",
        "transfer-soft-mask-pattern",
        "transfer-soft-mask-overprint",
    ] {
        let document = Document::open(
            fixture("recovery", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let pixels = list.render(&RenderOptions::default()).unwrap();
        assert_eq!(
            pagura_testkit::ppm(
                pixels.width(),
                pixels.height(),
                &pixels.to_rgb_over(pagura_raster::Rgba8::WHITE)
            ),
            fixture("recovery", &format!("{name}.ppm")),
            "{name}"
        );
        assert!(document.diagnostics().is_empty(), "{name}");
    }
}

/// Where a repair is placed: in the object's stream data, or, for a fault of
/// an image dictionary, at the image object itself.
#[derive(Clone, Copy, Debug)]
enum Placement {
    Stream,
    Object,
}

impl Placement {
    /// Checks that `position`, reported for case `name`, is this placement
    /// in object `id` of `document`.
    fn check(self, document: &Document, id: ObjectId, position: Position, name: &str) {
        match self {
            Self::Stream => assert!(
                matches!(position, Position::Stream { object, .. } if object == id),
                "{name}: {position:?}"
            ),
            Self::Object => {
                let ObjectSource::File(range) = document.object(id).unwrap().source() else {
                    panic!("{name}: object {id:?} is not at a file offset");
                };
                assert_eq!(position, Position::File(range.start), "{name}");
            }
        }
    }
}

#[test]
fn reported_repairs_reject_strictly_and_keep_their_context() {
    use Placement::{Object, Stream};
    for (name, tag, object, placement) in [
        ("unclosed-graphics", "content.graphics-unclosed", 4, Stream),
        ("type3-width-advance", "font.type3-width", 6, Stream),
        ("type3-vertical-metric", "font.type3-width", 6, Stream),
        (
            "pattern-coloured-underlying",
            "content.pattern-underlying",
            4,
            Stream,
        ),
        (
            "pattern-underlying-retained",
            "content.pattern-underlying",
            4,
            Stream,
        ),
        (
            "lzw-no-initial-clear",
            "filter.lzw-initial-clear",
            5,
            Stream,
        ),
        ("jpeg-trailing-restart", "jpeg.trailing-restart", 5, Stream),
        ("ccitt-cropped-columns", "image.ccitt-columns", 5, Object),
        (
            "predictor-image-precision",
            "image.predictor-precision",
            5,
            Object,
        ),
        ("jpx-part-count", "jpx.tile-part-count", 5, Stream),
        (
            "jpx-misplaced-mapping",
            "jpx.mapping-box-location",
            5,
            Stream,
        ),
        ("image-xobject-type", "image.xobject-type", 5, Object),
        (
            "image-flate-mislabeled-dct",
            "image.filter-mismatch",
            5,
            Stream,
        ),
    ] {
        let id = ObjectId::new(object, 0);
        let strict = Document::open(
            fixture("recovery", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert_eq!(
            error.repair.map(pagura_core::Repair::as_str),
            Some(tag),
            "{name}"
        );
        assert_eq!(error.object, Some(id), "{name}");
        placement.check(&strict, id, error.position, name);
        assert!(strict.diagnostics().is_empty(), "{name}");
        let repaired = Document::open(
            fixture("recovery", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
        let pixels = list.render(&RenderOptions::default()).unwrap();
        assert_eq!(
            pagura_testkit::ppm(
                pixels.width(),
                pixels.height(),
                &pixels.to_rgb_over(pagura_raster::Rgba8::WHITE)
            ),
            fixture("recovery", &format!("{name}.ppm")),
            "{name}"
        );
        let diagnostics = repaired.diagnostics();
        assert_eq!(diagnostics.len(), 1, "{name}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].repair().map(pagura_core::Repair::as_str),
            Some(tag),
            "{name}"
        );
        assert_eq!(diagnostics[0].object, Some(id), "{name}");
        compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
        assert_eq!(repaired.diagnostics(), diagnostics, "{name}");
    }
}

#[test]
fn transfer_selection_is_tile_independent() {
    for name in [
        "transfer-opaque",
        "transfer-translucent-default",
        "transfer-reset-by-topmost",
        "transfer-identity-topmost",
        "transfer-transparent-topmost",
        "transfer-opaque-ancestor",
        "transfer-translucent-ancestor",
        "transfer-image",
        "transfer-soft-mask-pattern",
        "transfer-soft-mask-overprint",
    ] {
        let doc = Document::open(
            fixture("recovery", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = compile_page(&doc, doc.page(0).unwrap()).unwrap();
        let pixels = list.render(&RenderOptions::default()).unwrap();
        assert_eq!(
            pagura_testkit::ppm(
                pixels.width(),
                pixels.height(),
                &pixels.to_rgb_over(pagura_raster::Rgba8::WHITE)
            ),
            fixture("recovery", &format!("{name}.ppm")),
            "{name}"
        );
        for options in [RenderOptions::default(), RenderOptions::from_dpi(144.0)] {
            let full = list.render(&options).unwrap();
            let mut assembled = vec![0; full.data().len()];
            for y in (0..full.height()).step_by(7) {
                for x in (0..full.width()).step_by(5) {
                    let tile = Tile {
                        x,
                        y,
                        width: 5.min(full.width() - x),
                        height: 7.min(full.height() - y),
                    };
                    let pixels = list.render_tile(&options, tile).unwrap();
                    for row in 0..tile.height {
                        let start = ((y + row) * full.width() + x) as usize * 4;
                        let source = row as usize * tile.width as usize * 4;
                        let len = tile.width as usize * 4;
                        assembled[start..start + len]
                            .copy_from_slice(&pixels.data()[source..source + len]);
                    }
                }
            }
            assert_eq!(assembled, full.data(), "{name}");
        }
    }
}
