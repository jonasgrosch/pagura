//! Functions in compositing: transfer functions selected per output pixel
//! (11.7.5.2), soft-mask transfer functions, black generation for group
//! results, and spot colorants under overprint simulation, against the
//! reference.

use crate::testing::{Build, check, mark};
use pagura_color::{ColorSpace, Colorant, ExponentialFunction, Function, Process, Separation};
use pagura_core::{ErrorKind, Limits, Name, Position};
use pagura_interpret::{
    GroupAttributes, GroupId, MarkInfo, MaskKind, Overprint, StateId, Transfer,
};
use std::sync::Arc;

const PAGE: GroupId = GroupId(0);

/// `c0 + x^n (c1 - c0)` on `[0, 1]`.
fn exponential(c0: &[f64], c1: &[f64], exponent: f64) -> Arc<Function> {
    Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: c0.to_vec(),
            c1: c1.to_vec(),
            exponent,
        })
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

#[test]
fn the_last_opaque_mark_selects_its_transfer_function() {
    let mut b = Build::new(8.0, 4.0);
    let square = exponential(&[0.0], &[1.0], 2.0);
    let transferred = b.state(|s| s.transfer = Some(Arc::new(Transfer::All(Arc::clone(&square)))));
    let half = b.state(|s| s.alpha = 0.5);
    let teal = b.rgb([0.2, 0.6, 0.7]);
    let red = b.rgb([0.9, 0.3, 0.1]);
    b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], teal, mark(transferred));
    b.rect(PAGE, [4.0, 0.0, 8.0, 2.0], red, mark(StateId(0)));
    b.rect(PAGE, [2.0, 2.0, 5.0, 4.0], red, mark(half));
    check(&b);
}

#[test]
fn a_fractional_boundary_of_a_transfer_fails_preparation() {
    let mut b = Build::new(8.0, 4.0);
    let square = exponential(&[0.0], &[1.0], 2.0);
    let transferred = b.state(|s| s.transfer = Some(Arc::new(Transfer::All(square))));
    let teal = b.rgb([0.2, 0.6, 0.7]);
    b.rect(PAGE, [0.0, 0.0, 3.0, 4.0], teal, mark(StateId(0)));
    let info = MarkInfo {
        position: Position::Buffer(9),
        ..mark(transferred)
    };
    b.rect(PAGE, [0.5, 0.0, 6.0, 4.0], teal, info);
    for height in [1, 3, 32] {
        let error = b
            .prepare(&b.request(b.page(), height), &Limits::default())
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(pagura_interpret::features::TRANSPARENCY_TRANSFER)
        );
        assert_eq!(error.position, Position::Buffer(9));
    }
}

#[test]
fn soft_mask_transfer_functions_map_mask_values() {
    for kind in [MaskKind::Alpha, MaskKind::Luminosity] {
        let mut b = Build::new(6.0, 4.0);
        let (mask, group) = b.mask(kind, GroupAttributes::default(), None);
        b.scene.masks[0].transfer = Some(exponential(&[1.0], &[0.0], 0.5));
        let gray = b.rgb([0.7, 0.7, 0.7]);
        let half = b.state(|s| s.alpha = 0.5);
        b.rect(group, [1.0, 0.5, 5.5, 3.25], gray, mark(half));
        let masked = b.state(|s| s.soft_mask = Some(mask));
        let blue = b.rgb([0.1, 0.2, 0.8]);
        b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], blue, mark(masked));
        check(&b);
    }
}

#[test]
fn black_generation_converts_group_results() {
    let mut b = Build::new(6.0, 4.0);
    b.scene.groups[0].attributes.space = Some(Process::Cmyk);
    let cmyk = b.color(ColorSpace::DeviceCmyk, &[0.1, 0.2, 0.0, 0.1]);
    b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], cmyk, mark(StateId(0)));
    let group_state = b.state(|s| {
        s.alpha = 0.8;
        s.conversion.black_generation = Some(exponential(&[0.0], &[0.5], 1.0));
        s.conversion.undercolor_removal = Some(exponential(&[0.0], &[0.25], 1.0));
    });
    let attributes = GroupAttributes {
        isolated: true,
        knockout: false,
        space: Some(Process::Rgb),
    };
    let group = b.transparency(PAGE, attributes, mark(group_state));
    let rgb = b.rgb([0.25, 0.5, 0.75]);
    b.rect(group, [1.0, 0.5, 5.0, 3.5], rgb, mark(StateId(0)));
    check(&b);
}

#[test]
fn spot_colorants_overprint_under_simulation() {
    for overprint in [false, true] {
        let mut b = Build::new(6.0, 4.0);
        b.simulate = true;
        let tint = exponential(&[0.0, 0.0, 0.0, 0.0], &[0.1, 0.9, 0.4, 0.0], 1.0);
        let spot = ColorSpace::Separation(Arc::new(Separation {
            colorant: Colorant::Named(Name::from(b"Orange")),
            alternate: Arc::new(ColorSpace::DeviceCmyk),
            tint,
        }));
        let orange = b.color(spot, &[0.7]);
        let cmyk = b.color(ColorSpace::DeviceCmyk, &[0.0, 0.1, 0.0, 0.4]);
        let overprinting = b.state(|s| {
            s.overprint = Overprint {
                enabled: overprint,
                nonzero: true,
            };
        });
        b.rect(PAGE, [0.0, 0.0, 4.0, 4.0], cmyk, mark(StateId(0)));
        b.rect(PAGE, [2.0, 0.5, 6.0, 3.5], orange, mark(overprinting));
        b.rect(PAGE, [0.5, 2.0, 5.5, 4.0], cmyk, mark(overprinting));
        check(&b);
    }
}
