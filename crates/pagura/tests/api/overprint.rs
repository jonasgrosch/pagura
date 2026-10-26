//! Preview policies share one recorded scene and independent analytic
//! oracles.

use crate::support::{assert_tile_matches, open, ppm};
use pagura::{OverprintPreview, RenderContext, RenderOptions, Rgba8, Tile};
use pagura_testkit::{assert_bytes_eq, fixture};
use std::sync::Arc;

/// A context for an overprint fixture and its first page's scene.
fn recorded(name: &str) -> pagura::ReadResult<(RenderContext, Arc<pagura::Scene>)> {
    let document = open(fixture("overprint", &format!("{name}.pdf")))?;
    let context = pagura::render_context(Arc::new(document));
    let scene = context.scene(context.document().page(0)?)?;
    Ok((context, scene))
}

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
        let (context, scene) = recorded(&format!("preview-{name}")).unwrap();
        // Revisit modes after a simulated rendering.
        for mode in [
            OverprintPreview::Always,
            OverprintPreview::Never,
            OverprintPreview::Auto,
            OverprintPreview::Always,
        ] {
            let options = RenderOptions {
                overprint: mode,
                ..RenderOptions::default()
            };
            let simulated =
                mode == OverprintPreview::Always || (mode == OverprintPreview::Auto && print);
            let oracle = if simulated {
                "pdfx-intent"
            } else {
                "no-intent"
            };
            let expected = fixture("overprint", &format!("preview-{oracle}.ppm"));
            let full = context.render_scene(&scene, &options, None).unwrap();
            assert_bytes_eq(&ppm(&full), &expected, &format!("{name} {mode:?}"));
            let tile = Tile {
                x: 3,
                y: 2,
                width: 2,
                height: 3,
            };
            let part = context.render_scene(&scene, &options, Some(tile)).unwrap();
            assert_tile_matches(&full, &part, tile);
        }
    }
}

#[test]
fn never_bypasses_simulated_plate_limits_even_after_a_failure() {
    let (context, scene) = recorded("spot-budget").unwrap();
    let error = context
        .render_scene(&scene, &RenderOptions::default(), None)
        .unwrap_err();
    assert_eq!(error.kind, pagura::ErrorKind::Limit);
    let never = RenderOptions {
        overprint: OverprintPreview::Never,
        ..RenderOptions::default()
    };
    let image = context.render_scene(&scene, &never, None).unwrap();
    assert_bytes_eq(
        &ppm(&image),
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
        let (context, scene) = recorded(name).unwrap();
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
            let options = RenderOptions {
                overprint: mode,
                ..RenderOptions::default()
            };
            let actual = context
                .render_scene(&scene, &options, None)
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
