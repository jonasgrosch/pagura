#![forbid(unsafe_code)]
//! Preview policies share compiled content and independent analytic oracles.

use pagura::{Document, OpenOptions, OverprintPreview, RenderOptions, Rgba8, Tile};
use pagura_testkit::{assert_bytes_eq, fixture, ppm};

#[test]
fn preview_modes_select_print_intent_without_changing_compilation() {
    assert_eq!(RenderOptions::default().overprint, OverprintPreview::Auto);
    for (name, print) in [
        ("pdfx-intent", true),
        ("cmyk-group", true),
        ("pdfx-info", true),
        ("indirect-intent", true),
        ("page-intent", true),
        ("page-overrides-catalog", false),
        ("empty-page-intents", false),
        ("indirect-page-intents", false),
        ("invalid-page-intents", true),
        ("unresolved-page-intents", true),
        ("page-intent-with-pdfx-info", true),
        ("no-intent", false),
        ("other-intent", false),
        ("malformed-intent", false),
        ("malformed-info", false),
    ] {
        let document = Document::open(
            fixture("overprint", &format!("preview-{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = pagura::compile_page(&document, document.page(0).unwrap()).unwrap();
        // Revisit modes after the simulated plan has been cached.
        for mode in [
            OverprintPreview::Always,
            OverprintPreview::Never,
            OverprintPreview::Auto,
            OverprintPreview::Always,
        ] {
            let options = RenderOptions::default().with_overprint(mode);
            let simulated =
                mode == OverprintPreview::Always || (mode == OverprintPreview::Auto && print);
            let oracle = if simulated {
                "pdfx-intent"
            } else {
                "no-intent"
            };
            let expected = fixture("overprint", &format!("preview-{oracle}.ppm"));
            let full = list.render(&options).unwrap();
            assert_bytes_eq(
                &ppm(8, 8, &full.to_rgb_over(Rgba8::WHITE)),
                &expected,
                &format!("{name} {mode:?}"),
            );
            let tile = list
                .render_tile(
                    &options,
                    Tile {
                        x: 3,
                        y: 2,
                        width: 2,
                        height: 3,
                    },
                )
                .unwrap();
            for row in 0..3 {
                assert_eq!(
                    &tile.data()[row * 8..(row + 1) * 8],
                    &full.data()[(row + 2) * 32 + 12..(row + 2) * 32 + 20]
                );
            }
        }
    }
}

#[test]
fn never_bypasses_simulated_plate_limits_even_after_a_cached_failure() {
    let document = Document::open(
        fixture("overprint", "spot-budget.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let list = pagura::compile_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(
        list.render(&RenderOptions::default()).unwrap_err().kind,
        pagura::ErrorKind::Limit
    );
    let image = list
        .render(&RenderOptions::default().with_overprint(OverprintPreview::Never))
        .unwrap();
    assert_bytes_eq(
        &ppm(8, 8, &image.to_rgb_over(Rgba8::WHITE)),
        &fixture("overprint", "spot-budget-no-intent.ppm"),
        "Never ignores the simulated plate budget",
    );
}

#[test]
fn preview_policy_controls_cmyk_luminosity_masks_without_page_separations() {
    for (name, print) in [
        ("preview-cmyk-mask", true),
        ("preview-cmyk-mask-no-intent", false),
    ] {
        let document = Document::open(
            fixture("overprint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = pagura::compile_page(&document, document.page(0).unwrap()).unwrap();
        for mode in [
            OverprintPreview::Never,
            OverprintPreview::Auto,
            OverprintPreview::Always,
        ] {
            let simulated =
                mode == OverprintPreview::Always || (mode == OverprintPreview::Auto && print);
            let oracle = if simulated {
                "preview-cmyk-mask"
            } else {
                "preview-cmyk-mask-no-intent"
            };
            let actual = list
                .render(&RenderOptions::default().with_overprint(mode))
                .unwrap()
                .to_rgb_over(Rgba8::WHITE);
            let expected = fixture("overprint", &format!("{oracle}.ppm"));
            for (a, e) in actual
                .iter()
                .zip(&expected[expected.len() - actual.len()..])
            {
                assert!(a.abs_diff(*e) <= 1, "{name} {mode:?}: {a} != {e}");
            }
        }
    }
}
