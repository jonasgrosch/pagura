//! Ownership and construction admission of the final image sampling plan.

use super::super::{
    Description, ImageBudgets, MaskDescription, Paint, Plan, geometry, stencil_paint,
};
use super::{converter, image, meter};
use pagura_color::{ColorSpace, Painted};
use pagura_core::Meter;
use pagura_image::{ImageKind, Samples};
use pagura_raster::Transform;

#[test]
fn rgb_codes_drop_the_original_converter_and_tables() {
    let image = image((2, 2), 3, 8, Samples::Bytes(vec![0; 12]));
    let geometry = geometry::Geometry::new((2, 2), false, Transform::scale(0.125, 0.125)).unwrap();
    let mut bytes = meter();
    let mut functions = meter();
    let colors = super::super::colors(
        &Description {
            space: Some(&ColorSpace::DeviceRgb),
            decode: &[[0.0, 1.0]; 3],
            key: None,
            mask: MaskDescription::None,
            alpha_is_shape: false,
        },
        &image,
        None,
        converter(&ColorSpace::DeviceRgb, 3).0,
        &mut bytes,
    )
    .unwrap();
    let strategy =
        super::super::Strategy::new(&image, &colors, true, None, &mut bytes, &mut functions)
            .unwrap();
    bytes
        .charge_items::<(super::super::Colors, super::super::Strategy)>(1)
        .unwrap();
    let paint = Paint::Colors(Box::new((colors, strategy)));
    let released = paint.bytes();
    assert!(released >= 3 * 256 * size_of::<f64>());
    assert_eq!(bytes.used(), released);
    let before_functions = functions.used();
    let (plan, painted) = Plan::new(
        (paint, Painted(7)),
        &image,
        (&geometry, (0, 8)),
        None,
        None,
        64,
        ImageBudgets {
            bytes: &mut bytes,
            work: &mut meter(),
            functions: &mut functions,
        },
    )
    .unwrap();
    assert!(matches!(plan, Plan::Codes(_)));
    assert_eq!(painted, Painted(7));
    assert_eq!(bytes.used(), plan.bytes());
    assert_eq!(functions.used(), before_functions);
    eprintln!("released construction paint heap bytes: {released}");
}

#[test]
fn non_affine_stencil_keeps_its_selected_bilinear_separable() {
    let mut image = image((2, 2), 1, 1, Samples::Bytes(vec![0, 1, 1, 0]));
    image.kind = ImageKind::Stencil { inverted: false };
    let geometry = geometry::Geometry::new((2, 2), true, Transform::scale(0.125, 0.125)).unwrap();
    assert!(matches!(
        super::super::Sampling::new(&geometry, (0, 8), &mut meter()).unwrap(),
        super::super::Sampling::Points,
    ));
    let mut bytes = meter();
    let paint = stencil_paint(&image, &mut bytes).unwrap();
    let (plan, _) = Plan::new(
        (paint, Painted(0)),
        &image,
        (&geometry, (0, 8)),
        None,
        Some((&[f64::INFINITY, 0.5, 0.0], Painted(7))),
        64,
        ImageBudgets {
            bytes: &mut bytes,
            work: &mut meter(),
            functions: &mut meter(),
        },
    )
    .unwrap();
    assert!(matches!(
        plan,
        Plan::Texels {
            sampling: super::super::Sampling::Separable(_),
            ..
        }
    ));
    assert_eq!(bytes.used(), plan.bytes());
}

#[test]
fn work_refusal_precedes_stencil_recolouring_allocation() {
    let mut image = image((2, 2), 1, 1, Samples::Bytes(vec![0, 1, 1, 0]));
    image.kind = ImageKind::Stencil { inverted: false };
    let geometry = geometry::Geometry::new((2, 2), false, Transform::scale(0.125, 0.125)).unwrap();
    let mut bytes = meter();
    let paint = stencil_paint(&image, &mut bytes).unwrap();
    let table = paint.bytes();
    let (plan, _) = Plan::new(
        (paint, Painted(0)),
        &image,
        (&geometry, (0, 8)),
        None,
        None,
        1,
        ImageBudgets {
            bytes: &mut bytes,
            work: &mut meter(),
            functions: &mut meter(),
        },
    )
    .unwrap();
    let before_recolouring = plan.bytes() + table;
    for (work_limit, message) in [(0, "work admission"), (usize::MAX, "byte admission")] {
        let mut bytes = Meter::new(before_recolouring, "byte admission");
        let paint = stencil_paint(&image, &mut bytes).unwrap();
        let error = Plan::new(
            (paint, Painted(0)),
            &image,
            (&geometry, (0, 8)),
            None,
            Some((&[0.25, 0.5, 0.75], Painted(7))),
            1,
            ImageBudgets {
                bytes: &mut bytes,
                work: &mut Meter::new(work_limit, "work admission"),
                functions: &mut meter(),
            },
        )
        .unwrap_err();
        assert_eq!(error.message, message);
    }
}

#[test]
fn code_table_function_fees_remain_after_converter_is_dropped() {
    use pagura_color::{
        Colorant, Conversion, Converter, ExponentialFunction, Function, Process, Separation, Target,
    };
    use pagura_core::Name;
    use std::sync::Arc;

    let space = ColorSpace::Separation(Arc::new(Separation {
        colorant: Colorant::Named(Name::new(b"Ink")),
        alternate: Arc::new(ColorSpace::DeviceGray),
        tint: Arc::new(
            Function::exponential(ExponentialFunction {
                domain: [0.0, 1.0],
                range: None,
                c0: vec![0.0],
                c1: vec![1.0],
                exponent: 1.0,
            })
            .unwrap(),
        ),
    }));
    for admit in [true, false] {
        let decoded = Arc::new(pagura_image::Decoded::from(image(
            (2, 2),
            1,
            8,
            Samples::Bytes(vec![0, 1, 2, 3]),
        )));
        let mut bytes = meter();
        bytes
            .charge(super::super::decoded_bytes(&decoded.image))
            .unwrap();
        let convert = Converter::new_metered(
            &space,
            &Target::new(Process::Rgb),
            &Conversion::default(),
            &mut bytes,
        )
        .unwrap();
        let fees = 256 * convert.cost();
        assert!(fees > 0);
        let mut functions = Meter::new(fees - usize::from(!admit), "function admission");
        let result = super::super::Prepared::new(
            &Description {
                space: Some(&space),
                decode: &[[0.0, 1.0]],
                key: None,
                mask: MaskDescription::None,
                alpha_is_shape: false,
            },
            (decoded, None),
            (
                Transform::scale(0.125, 0.125),
                (0, 8),
                super::super::Orientation::Rows,
            ),
            Some((convert, false)),
            None,
            64,
            ImageBudgets {
                bytes: &mut bytes,
                work: &mut meter(),
                functions: &mut functions,
            },
        );
        if admit {
            let prepared = result.unwrap();
            assert!(matches!(prepared.plan, Plan::Codes(_)));
            assert_eq!(functions.used(), fees);
            assert_eq!(bytes.used(), prepared.bytes());
        } else {
            assert_eq!(result.unwrap_err().message, "function admission");
        }
    }
}
