//! Portable frozen full-pixel and nine lower-budget contracts, without env paths.

use super::packed_index::{pixels, scene};
use crate::{RenderRequest, prepare};
use pagura_core::{Error, ErrorKind, Limits, ObjectId, Position, Store};
use pagura_raster::Rgba8;
use pagura_testkit::fixture;

#[test]
fn all_five_minification_oracles_both_backgrounds() {
    for name in [
        "minified-forty-rows",
        "minified-forty-mirror",
        "minified-forty-quarter",
        "minified-forty-256canvas",
        "highwidth-short-crop",
    ] {
        for (background, suffix) in [(Rgba8::WHITE, "white"), (Rgba8::TRANSPARENT, "transparent")] {
            let source = scene(name, &Limits::default());
            let mut request = RenderRequest::page(&source.page, 1.0).unwrap();
            request.background = background;
            let store = Store::new(0);
            let prepared = prepare(&source, &request, &store, &Limits::default()).unwrap();
            let actual = pixels(&prepared);
            let expected = fixture("packed_mask_tiles", &format!("{name}-rgba.{suffix}.rgba"));
            assert_eq!(actual.len(), expected.len());
            let worst = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(worst <= 1, "{name} {suffix}: {worst}");
            assert_eq!(store.retained_bytes(), 0);
        }
    }
}

struct Profile {
    name: &'static str,
    original: Limits,
    caller: Limits,
    outcome: Outcome,
}

enum Outcome {
    Render,
    OldLimit(&'static str, usize),
    LateLimit,
}

fn profiles() -> [Profile; 9] {
    let defaults = Limits::default();
    [
        Profile {
            name: "highwidth-short-crop",
            original: defaults,
            caller: Limits {
                render_bytes: 16 * 1024 * 1024,
                ..defaults
            },
            outcome: Outcome::Render,
        },
        Profile {
            name: "highwidth-short-crop",
            original: defaults,
            caller: Limits {
                render_work: 100_000_000,
                ..defaults
            },
            outcome: Outcome::Render,
        },
        Profile {
            name: "highwidth-short-crop",
            original: defaults,
            caller: Limits {
                render_bytes: 4 * 1024 * 1024,
                ..defaults
            },
            outcome: Outcome::OldLimit("render memory budget exceeded", 48),
        },
        Profile {
            name: "minified-forty-rows",
            original: defaults,
            caller: Limits {
                render_work: 1_400_000_000,
                ..defaults
            },
            outcome: Outcome::LateLimit,
        },
        Profile {
            name: "minified-forty-rows",
            original: Limits {
                render_work: 500_000_000,
                ..defaults
            },
            caller: defaults,
            outcome: Outcome::OldLimit("render work budget exceeded", 884),
        },
        Profile {
            name: "default-large",
            original: defaults,
            caller: Limits {
                render_work: 300_000_000,
                ..defaults
            },
            outcome: Outcome::Render,
        },
        Profile {
            name: "default-large",
            original: defaults,
            caller: Limits {
                render_bytes: 32 * 1024 * 1024,
                ..defaults
            },
            outcome: Outcome::Render,
        },
        Profile {
            name: "default-large",
            original: Limits {
                render_work: 300_000_000,
                ..defaults
            },
            caller: defaults,
            outcome: Outcome::Render,
        },
        Profile {
            name: "scene-forty-shared",
            original: defaults,
            caller: defaults,
            outcome: Outcome::Render,
        },
    ]
}

fn limit(error: &Error, outcome: &Outcome) {
    assert_eq!(error.kind, ErrorKind::Limit);
    assert!(error.feature().is_none() && error.repair.is_none());
    if let Outcome::OldLimit(message, offset) = outcome {
        let object = ObjectId::new(4, 0);
        assert_eq!(&error.message, message);
        assert_eq!(error.object, Some(object));
        assert_eq!(
            error.position,
            Position::Stream {
                object,
                offset: *offset
            }
        );
    }
    // The frozen late contract requires Limit/no output; its source context
    // may change after a successful private transition, never a new golden.
}

#[test]
fn nine_frozen_lower_budget_profiles() {
    for profile in profiles() {
        let source = scene(profile.name, &profile.original);
        let request = RenderRequest::page(&source.page, 1.0).unwrap();
        let store = Store::new(0);
        match prepare(&source, &request, &store, &profile.caller) {
            Ok(prepared) => {
                assert!(matches!(profile.outcome, Outcome::Render));
                let actual = pixels(&prepared);
                let expected = fixture(
                    "packed_mask_tiles",
                    &format!("{}-rgba.white.rgba", profile.name),
                );
                assert_eq!(actual.len(), expected.len());
                assert!(actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1));
            }
            Err(error) => {
                assert!(
                    !matches!(profile.outcome, Outcome::Render),
                    "{}: {error}",
                    profile.name
                );
                limit(&error, &profile.outcome);
            }
        }
        assert_eq!(store.retained_bytes(), 0);
    }
}
