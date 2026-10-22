use super::*;

const RGB: Blending = Blending {
    mode: BlendMode::Normal,
    process: Process::Rgb,
    channels: 3,
};

fn source(color: &[f64], alpha: f64, shape: f64) -> Source {
    let mut c = [0.0; MAX];
    c[..color.len()].copy_from_slice(color);
    Source {
        color: c,
        alpha,
        shape,
        preserve: 0,
    }
}

fn close(actual: &[f64], expected: &[f64]) {
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!((a - e).abs() < 1e-15, "channel {i}: {a} against {e}");
    }
}

#[test]
fn normal_source_over_mixes_by_alpha() {
    let mut pixel = Pixel::opaque(&[1.0, 1.0, 1.0]);
    pixel.paint(
        &source(&[1.0, 0.0, 0.0], 0.5, 1.0),
        RGB,
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    close(&pixel.color[..3], &[1.0, 0.5, 0.5]);
    assert_eq!((pixel.alpha, pixel.group, pixel.shape), (1.0, 0.5, 1.0));

    // Onto transparency: premultiplied colour, and group alpha and shape
    // accumulate as unions: 1 - (1 - a)(1 - b).
    let mut pixel = Pixel::TRANSPARENT;
    pixel.paint(
        &source(&[0.2, 0.4, 0.6], 0.5, 0.75),
        RGB,
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    close(&pixel.color[..3], &[0.1, 0.2, 0.3]);
    pixel.paint(
        &source(&[0.2, 0.4, 0.6], 0.5, 0.75),
        RGB,
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    assert_eq!(
        (pixel.alpha, pixel.group, pixel.shape),
        (0.75, 0.75, 0.9375)
    );
    close(&pixel.straight(3)[..3], &[0.2, 0.4, 0.6]);
}

#[test]
fn blend_modes_mix_the_blend_result_by_backdrop_alpha() {
    // 11.3.6: C = (1 - as) Cb + as ((1 - ab) Cs + ab B(Cb, Cs)), here with
    // ab = 1/2, so the backdrop's premultiplied colour enters.
    let multiply = Blending {
        mode: BlendMode::Multiply,
        ..RGB
    };
    let mut pixel = Pixel::TRANSPARENT;
    pixel.paint(
        &source(&[0.5, 1.0, 0.25], 0.5, 1.0),
        RGB,
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    pixel.paint(
        &source(&[0.5, 0.5, 0.5], 0.5, 1.0),
        multiply,
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    let (ab, r#as) = (0.5, 0.5);
    let expected: Vec<f64> = [(0.5, 0.5), (1.0, 0.5), (0.25, 0.5)]
        .iter()
        .map(|&(b, s)| (1.0 - r#as) * b * ab + r#as * ((1.0 - ab) * s + ab * b * s))
        .collect();
    close(&pixel.color[..3], &expected);
    assert_eq!(pixel.alpha, 0.75);
}

#[test]
fn knockout_replaces_earlier_elements_by_shape() {
    let initial = Pixel::opaque(&[1.0, 1.0, 1.0]);
    let mut pixel = initial;
    pixel.paint(
        &source(&[1.0, 0.0, 0.0], 1.0, 1.0),
        RGB,
        &initial,
        Elements::Knockout,
    );
    pixel.paint(
        &source(&[0.0, 0.0, 1.0], 0.5, 1.0),
        RGB,
        &initial,
        Elements::Knockout,
    );
    // The blue element composites with white, not with red.
    close(&pixel.color[..3], &[0.5, 0.5, 1.0]);
    // A half shape keeps half of the earlier result; alpha / shape is the
    // element's opacity over the initial backdrop.
    let mut pixel = initial;
    pixel.paint(
        &source(&[1.0, 0.0, 0.0], 1.0, 1.0),
        RGB,
        &initial,
        Elements::Knockout,
    );
    pixel.paint(
        &source(&[0.0, 0.0, 1.0], 0.25, 0.5),
        RGB,
        &initial,
        Elements::Knockout,
    );
    let over_white = [0.5, 0.5, 1.0];
    let expected: Vec<f64> = [1.0, 0.0, 0.0]
        .iter()
        .zip(over_white)
        .map(|(r, w)| 0.5 * r + 0.5 * w)
        .collect();
    close(&pixel.color[..3], &expected);
    assert_eq!(pixel.group, 0.5 * 1.0 + 0.25);
}

#[test]
fn overprinted_channels_keep_the_backdrop() {
    let cmyk = Blending {
        mode: BlendMode::Normal,
        process: Process::Cmyk,
        channels: 4,
    };
    // Stored CMYK is 1 - tint: the backdrop has 40% black.
    let mut pixel = Pixel::opaque(&[1.0, 1.0, 1.0, 0.6]);
    let mut s = source(&[0.0, 1.0, 1.0, 1.0], 1.0, 1.0);
    s.preserve = 0b1110;
    pixel.paint(&s, cmyk, &Pixel::TRANSPARENT, Elements::Accumulate);
    close(&pixel.color[..4], &[0.0, 1.0, 1.0, 0.6]);
    // Under Multiply, the preserved channel enters the blend from the
    // backdrop: B(0.6, 0.6) = 0.36.
    let mut pixel = Pixel::opaque(&[1.0, 1.0, 1.0, 0.6]);
    pixel.paint(
        &s,
        Blending {
            mode: BlendMode::Multiply,
            ..cmyk
        },
        &Pixel::TRANSPARENT,
        Elements::Accumulate,
    );
    close(&pixel.color[..4], &[0.0, 1.0, 1.0, 0.36]);
}

#[test]
fn constant_alpha_and_masks_scale_alpha_and_with_ais_shape() {
    let mut state = Composite {
        alpha: 0.5,
        ..Composite::default()
    };
    let mut s = source(&[1.0], 0.8, 0.8);
    s.apply(&state, 0.5);
    assert_eq!((s.alpha, s.shape), (0.2, 0.8));
    state.alpha_is_shape = true;
    let mut s = source(&[1.0], 0.8, 0.8);
    s.apply(&state, 0.5);
    assert_eq!((s.alpha, s.shape), (0.2, 0.2));
}
