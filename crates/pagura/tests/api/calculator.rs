//! Shared function resources and pre-callback function-work admission.

use crate::support::ppm;
use pagura::{
    Document, ErrorKind, Limits, OpenOptions, Recovery, RenderContext, RenderOptions, Rgba8,
};
use pagura_testkit::fixture;
use std::sync::Arc;

#[test]
fn later_page_uses_its_own_value_with_the_same_shared_function() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        for capacity in [0, 4096, RenderContext::DEFAULT_STORE_BYTES] {
            let original = fixture("calculator_branches", "shared-function-pages.pdf");
            let document = Arc::new(
                Document::open(
                    original.clone(),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    },
                )
                .unwrap(),
            );
            let context = RenderContext::with_store_capacity(Arc::clone(&document), None, capacity);
            for _ in 0..2 {
                for (page, oracle) in [
                    (0, "shared-function-pages.ppm"),
                    (1, "shared-function-pages-p1.ppm"),
                ] {
                    let options = RenderOptions {
                        background: Rgba8::TRANSPARENT,
                        ..RenderOptions::from_dpi(72.0)
                    };
                    let pixels = context
                        .render_page(document.page(page).unwrap(), &options)
                        .unwrap();
                    assert_eq!(ppm(&pixels), fixture("calculator_branches", oracle));
                    assert!(pixels.data().chunks_exact(4).all(|pixel| pixel[3] == 255));
                }
            }
            assert_eq!(document.original_bytes(), original);
            assert!(document.diagnostics().is_empty());
            assert!(context.retained_bytes() <= capacity);
        }
    }
}

#[test]
fn zero_function_work_refuses_before_callbacks_with_stable_store() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        for capacity in [0, 4096, RenderContext::DEFAULT_STORE_BYTES] {
            let document = Arc::new(
                Document::open(
                    fixture("calculator_branches", "piecewise-axial.pdf"),
                    &OpenOptions {
                        recovery,
                        limits: Limits {
                            function_work: 0,
                            ..Limits::default()
                        },
                        ..OpenOptions::default()
                    },
                )
                .unwrap(),
            );
            let context = RenderContext::with_store_capacity(Arc::clone(&document), None, capacity);
            let mut expected = None;
            let mut retained = None;
            for _ in 0..2 {
                let mut callbacks = 0;
                let result = match context.scene(document.page(0).unwrap()) {
                    Ok(scene) => context.render_bands(
                        &scene,
                        &RenderOptions::from_dpi(72.0),
                        None,
                        |_| false,
                        |_, _| {
                            callbacks += 1;
                            Ok::<(), ()>(())
                        },
                    ),
                    Err(error) => Err(error),
                };
                let error = result.unwrap_err();
                assert_eq!(error.kind, ErrorKind::Limit);
                assert_eq!(callbacks, 0);
                let full = format!("{error:?}");
                if let Some(previous) = expected {
                    assert_eq!(full, previous);
                }
                expected = Some(full);
                let current = context.retained_bytes();
                assert!(current <= capacity);
                if let Some(previous) = retained {
                    assert_eq!(current, previous);
                }
                retained = Some(current);
            }
        }
    }
}
