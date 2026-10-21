//! Blend functions (11.3.5, Tables 134–135) in additive coordinates, one
//! zero-sized type per mode, so that span kernels are monomorphized over the
//! mode and decide nothing per pixel.

use pagura_interpret::BlendMode;

/// A blend mode as the kernels use it.
pub(crate) trait Blend {
    /// Normal: the blend result is the source colour.
    const NORMAL: bool;
    /// The mode works per component.
    const SEPARABLE: bool;
    /// The mode spot colorants composite with: Normal for Difference,
    /// Exclusion and the non-separable modes, the mode itself otherwise.
    type Spot: Blend;

    /// `B(b, s)` of one component of a separable mode.
    fn component(back: f64, source: f64) -> f64;

    /// `B(b, s)` of the `count` process components (1 Gray, 3 RGB, 4 CMYK).
    fn process(back: &[f64; 4], source: &[f64; 4], count: usize) -> [f64; 4] {
        let _ = count;
        [0, 1, 2, 3].map(|i| Self::component(back[i], source[i]))
    }
}

/// Something done with a blend mode chosen at run time: the kernels are
/// instantiated per mode through it.
pub(crate) trait BlendVisitor {
    /// The result.
    type Output;
    /// Runs with the mode's type.
    fn visit<B: Blend>(self) -> Self::Output;
}

/// Runs `visitor` with the type of `mode`.
pub(crate) fn with_blend<V: BlendVisitor>(mode: BlendMode, visitor: V) -> V::Output {
    match mode {
        BlendMode::Normal => visitor.visit::<Normal>(),
        BlendMode::Multiply => visitor.visit::<Multiply>(),
        BlendMode::Screen => visitor.visit::<Screen>(),
        BlendMode::Overlay => visitor.visit::<Overlay>(),
        BlendMode::Darken => visitor.visit::<Darken>(),
        BlendMode::Lighten => visitor.visit::<Lighten>(),
        BlendMode::ColorDodge => visitor.visit::<ColorDodge>(),
        BlendMode::ColorBurn => visitor.visit::<ColorBurn>(),
        BlendMode::HardLight => visitor.visit::<HardLight>(),
        BlendMode::SoftLight => visitor.visit::<SoftLight>(),
        BlendMode::Difference => visitor.visit::<Difference>(),
        BlendMode::Exclusion => visitor.visit::<Exclusion>(),
        BlendMode::Hue => visitor.visit::<Hue>(),
        BlendMode::Saturation => visitor.visit::<Saturation>(),
        BlendMode::Color => visitor.visit::<Color>(),
        BlendMode::Luminosity => visitor.visit::<Luminosity>(),
    }
}

macro_rules! separable {
    ($($(#[$doc:meta])* $name:ident, $spot:ty, |$b:ident, $s:ident| $body:expr;)*) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy)]
        pub(crate) struct $name;

        impl Blend for $name {
            const NORMAL: bool = false;
            const SEPARABLE: bool = true;
            type Spot = $spot;

            fn component($b: f64, $s: f64) -> f64 {
                $body
            }
        }
    )*};
}

/// Normal: the source.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Normal;

impl Blend for Normal {
    const NORMAL: bool = true;
    const SEPARABLE: bool = true;
    type Spot = Self;

    fn component(_back: f64, source: f64) -> f64 {
        source
    }
}

separable! {
    /// Multiply.
    Multiply, Self, |b, s| b * s;
    /// Screen.
    Screen, Self, |b, s| b + s - b * s;
    /// Overlay: HardLight with the operands swapped.
    Overlay, Self, |b, s| hard(s, b);
    /// Darken.
    Darken, Self, |b, s| b.min(s);
    /// Lighten.
    Lighten, Self, |b, s| b.max(s);
    /// ColorDodge, with PDF 2.0's boundary cases tested before the divisor.
    ColorDodge, Self, |b, s| {
        if b == 0.0 {
            0.0
        } else if b >= 1.0 - s {
            1.0
        } else {
            b / (1.0 - s)
        }
    };
    /// ColorBurn, with PDF 2.0's boundary cases tested before the divisor.
    ColorBurn, Self, |b, s| {
        if b == 1.0 {
            1.0
        } else if 1.0 - b >= s {
            0.0
        } else {
            1.0 - (1.0 - b) / s
        }
    };
    /// HardLight.
    HardLight, Self, |b, s| hard(b, s);
    /// SoftLight.
    SoftLight, Self, |b, s| {
        if s <= 0.5 {
            b - (1.0 - 2.0 * s) * b * (1.0 - b)
        } else {
            let d = if b <= 0.25 {
                ((16.0 * b - 12.0) * b + 4.0) * b
            } else {
                b.sqrt()
            };
            b + (2.0 * s - 1.0) * (d - b)
        }
    };
    /// Difference.
    Difference, Normal, |b, s| (b - s).abs();
    /// Exclusion.
    Exclusion, Normal, |b, s| b + s - 2.0 * b * s;
}

fn hard(b: f64, s: f64) -> f64 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}

macro_rules! nonseparable {
    ($($(#[$doc:meta])* $name:ident, |$b:ident, $s:ident| $body:expr, $black:literal;)*) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy)]
        pub(crate) struct $name;

        impl Blend for $name {
            const NORMAL: bool = false;
            const SEPARABLE: bool = false;
            type Spot = Normal;

            fn component(_back: f64, source: f64) -> f64 {
                source
            }

            fn process(back: &[f64; 4], source: &[f64; 4], count: usize) -> [f64; 4] {
                let rgb = |c: &[f64; 4]| {
                    if count == 1 {
                        [c[0]; 3]
                    } else {
                        [c[0], c[1], c[2]]
                    }
                };
                let ($b, $s) = (rgb(back), rgb(source));
                let result: [f64; 3] = $body;
                if count == 1 {
                    [lum(result), 0.0, 0.0, 0.0]
                } else {
                    // CMYK keeps the backdrop's black, or the source's for
                    // Luminosity (11.3.5.3).
                    let black = if $black { source[3] } else { back[3] };
                    [result[0], result[1], result[2], black]
                }
            }
        }
    )*};
}

nonseparable! {
    /// Hue.
    Hue, |b, s| set_lum(set_sat(s, saturation(b)), lum(b)), false;
    /// Saturation.
    Saturation, |b, s| set_lum(set_sat(b, saturation(s)), lum(b)), false;
    /// Color.
    Color, |b, s| set_lum(s, lum(b)), false;
    /// Luminosity.
    Luminosity, |b, s| set_lum(b, lum(s)), true;
}

/// The luminosity of an RGB colour (11.3.5.3).
pub(crate) fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn saturation(c: [f64; 3]) -> f64 {
    c.into_iter().fold(f64::NEG_INFINITY, f64::max) - c.into_iter().fold(f64::INFINITY, f64::min)
}

fn set_lum(c: [f64; 3], target: f64) -> [f64; 3] {
    let d = target - lum(c);
    let mut c = c.map(|v| v + d);
    let low = c.into_iter().fold(f64::INFINITY, f64::min);
    let high = c.into_iter().fold(f64::NEG_INFINITY, f64::max);
    if low < 0.0 {
        c = c.map(|v| target + (v - target) * target / (target - low));
    }
    if high > 1.0 {
        c = c.map(|v| target + (v - target) * (1.0 - target) / (high - target));
    }
    c.map(|v| v.clamp(0.0, 1.0))
}

fn set_sat(c: [f64; 3], amount: f64) -> [f64; 3] {
    let mut order = [0, 1, 2];
    order.sort_unstable_by(|&a, &b| c[a].total_cmp(&c[b]));
    let [low, middle, high] = order;
    let mut result = [0.0; 3];
    if c[high] > c[low] {
        result[middle] = (c[middle] - c[low]) * amount / (c[high] - c[low]);
        result[high] = amount;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Apply([f64; 4], [f64; 4], usize);

    impl BlendVisitor for Apply {
        type Output = [f64; 4];
        fn visit<B: Blend>(self) -> [f64; 4] {
            B::process(&self.0, &self.1, self.2)
        }
    }

    #[test]
    fn separable_modes_follow_table_134() {
        let cases = [
            (BlendMode::Multiply, 0.25 * 0.5),
            (BlendMode::Screen, 0.25 + 0.5 - 0.125),
            (BlendMode::Overlay, 2.0 * 0.25 * 0.5),
            (BlendMode::Darken, 0.25),
            (BlendMode::Lighten, 0.5),
            (BlendMode::ColorDodge, 0.5),
            (BlendMode::ColorBurn, 0.0),
            (BlendMode::HardLight, 2.0 * 0.25 * 0.5),
            (BlendMode::Difference, 0.25),
            (BlendMode::Exclusion, 0.25 + 0.5 - 2.0 * 0.125),
        ];
        for (mode, expected) in cases {
            let result = with_blend(mode, Apply([0.25; 4], [0.5; 4], 3));
            assert_eq!(result[0], expected, "{mode:?}");
        }
        let soft = with_blend(BlendMode::SoftLight, Apply([0.25; 4], [0.75; 4], 3))[0];
        let d = ((16.0 * 0.25 - 12.0) * 0.25 + 4.0) * 0.25;
        assert_eq!(soft, 0.25 + 0.5 * (d - 0.25));
    }

    #[test]
    fn nonseparable_modes_keep_luminosity_and_black() {
        let back = [0.2, 0.4, 0.6, 0.3];
        let source = [0.9, 0.1, 0.5, 0.8];
        let result = with_blend(BlendMode::Luminosity, Apply(back, source, 4));
        let expected = lum([source[0], source[1], source[2]]);
        assert!((lum([result[0], result[1], result[2]]) - expected).abs() < 1e-12);
        assert_eq!(result[3], source[3]);
        assert_eq!(
            with_blend(BlendMode::Color, Apply(back, source, 4))[3],
            back[3]
        );
        let gray = with_blend(BlendMode::Hue, Apply([0.4; 4], [0.7; 4], 1));
        assert!((gray[0] - 0.4).abs() < 1e-12);
    }
}
