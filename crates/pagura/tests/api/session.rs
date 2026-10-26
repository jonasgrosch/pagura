//! Public resumable output against independently generated image oracles.

use crate::support::{open, ppm};
use pagura::{BandOutput, OutputLimits, Pixmap, RenderContext, RenderOptions, Tile};
use pagura_testkit::{Json, Manifest, fixture};
use std::sync::Arc;

#[test]
fn owned_sessions_match_every_classic_image_oracle_and_collected_surface() {
    let manifest = Manifest::load("classic");
    let mut cases = 0;
    for case in manifest
        .cases()
        .filter(|case| case.string("expected") == Some("render"))
    {
        let name = case.name();
        let context = RenderContext::new(Arc::new(
            open(fixture("classic", &format!("{name}.pdf"))).unwrap(),
        ));
        let page = context.document().page(0).unwrap();
        let scene = context.scene(page).unwrap();
        let dpi = case.get("dpi").and_then(Json::as_f64).unwrap();
        let options = RenderOptions::from_dpi(dpi);
        let expected = context.render_page(page, &options).unwrap();
        let mut job = context
            .render_session(
                &scene,
                &options,
                None,
                OutputLimits {
                    slots: 1,
                    ..OutputLimits::default()
                },
            )
            .unwrap();
        // The owned prepared job must not depend on context/document lifetime.
        drop(scene);
        drop(context);
        let mut rgba = Vec::new();
        let mut row = 0;
        loop {
            match job.advance() {
                BandOutput::Band(band) => {
                    assert_eq!(band.rows().start, row);
                    row = band.rows().end;
                    rgba.extend_from_slice(band.premultiplied_rgba());
                }
                BandOutput::Complete => break,
                other => panic!("{name}: unexpected output {other:?}"),
            }
        }
        assert_eq!(rgba, expected.data(), "{name}: collected parity");
        assert_eq!(job.output_bytes(), 0);
        let actual = ppm(&Pixmap::from_premultiplied(
            expected.width(),
            expected.height(),
            rgba,
            pagura::Limits::default(),
        )
        .unwrap());
        let oracle = fixture("classic", &format!("{name}.ppm"));
        let tolerance = case.get("tolerance").and_then(Json::as_u64).unwrap_or(0);
        assert_eq!(actual.len(), oracle.len(), "{name}: size");
        assert!(
            actual
                .iter()
                .zip(&oracle)
                .all(|(a, b)| u64::from(a.abs_diff(*b)) <= tolerance),
            "{name}: independent oracle"
        );
        cases += 1;
    }
    assert!(cases >= 18);
}

#[test]
fn owned_tile_rows_keep_device_coordinates_and_survive_cancellation() {
    let document = Arc::new(open(fixture("classic", "many-rectangles.pdf")).unwrap());
    let context = RenderContext::new(document);
    let page = context.document().page(0).unwrap();
    let options = RenderOptions::default();
    let scene = context.scene(page).unwrap();
    let tile = Tile {
        x: 96,
        y: 163,
        width: 128,
        height: 64,
    };
    let expected = context.render_tile(page, &options, tile).unwrap();
    let mut job = context
        .render_session(&scene, &options, Some(tile), OutputLimits::default())
        .unwrap();
    let BandOutput::Band(first) = job.advance() else {
        panic!("first tile band")
    };
    assert_eq!(first.rows().start, tile.y);
    assert_eq!(first.region().x0, tile.x);
    assert_eq!(first.region().width(), tile.width);
    assert_eq!(first.stride(), usize::try_from(tile.width).unwrap() * 4);
    job.cancel();
    assert!(matches!(job.advance(), BandOutput::Cancelled));
    assert_eq!(job.output_bytes(), first.retained_bytes());
    assert_eq!(
        first.premultiplied_rgba(),
        &expected.data()[..first.premultiplied_rgba().len()]
    );
    drop(first);
    assert_eq!(job.output_bytes(), 0);
}
