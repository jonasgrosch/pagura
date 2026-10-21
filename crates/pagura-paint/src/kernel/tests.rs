//! The f64 kernel against an independent copy of the earlier compositor's
//! `Pixel::paint`, bit for bit, and Normal source-over against exact
//! rational arithmetic.

use super::*;
use crate::blend::{Blend, BlendVisitor};
use crate::plan::Layout;
use pagura_color::Process;

/// The earlier compositor's pixel: four channels, alpha, group alpha and
/// shape, with its `paint` written out as it was.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Old {
    color: [f64; 4],
    alpha: f64,
    group: f64,
    shape: f64,
}

struct Apply(BlendMode, [f64; 4], [f64; 4], usize);

impl BlendVisitor for Apply {
    type Output = [f64; 4];
    fn visit<B: Blend>(self) -> [f64; 4] {
        if B::SEPARABLE {
            [0, 1, 2, 3].map(|i| B::component(self.1[i], self.2[i]))
        } else {
            B::process(&self.1, &self.2, self.3)
        }
    }
}

impl Old {
    fn straight(self) -> [f64; 4] {
        if self.alpha > 0.0 {
            self.color.map(|v| (v / self.alpha).clamp(0.0, 1.0))
        } else {
            [0.0; 4]
        }
    }

    #[expect(clippy::too_many_arguments, reason = "the earlier signature, spelled out")]
    fn paint(
        &mut self,
        color: [f64; 4],
        alpha: f64,
        shape: f64,
        preserve: [bool; 4],
        mode: BlendMode,
        initial: Self,
        knockout: bool,
        components: usize,
    ) {
        let base = if knockout { initial } else { *self };
        let opacity = if knockout && shape > 0.0 {
            alpha / shape
        } else {
            alpha
        };
        let back = if preserve.iter().any(|&v| v) {
            base.straight()
        } else {
            [0.0; 4]
        };
        let (color, blend) = if mode == BlendMode::Normal {
            (
                color,
                [0, 1, 2, 3].map(|i| if preserve[i] { back[i] } else { color[i] }),
            )
        } else {
            let color = [0, 1, 2, 3].map(|i| {
                if preserve[i] {
                    (1.0 - base.alpha) * color[i] + base.alpha * back[i]
                } else {
                    color[i]
                }
            });
            (
                color,
                crate::blend::with_blend(mode, Apply(mode, base.straight(), color, components)),
            )
        };
        let value = [0, 1, 2, 3].map(|i| {
            (1.0 - opacity) * base.color[i]
                + opacity * ((1.0 - base.alpha) * color[i] + base.alpha * blend[i])
        });
        let value_alpha = opacity + base.alpha * (1.0 - opacity);
        if knockout {
            self.color = [0, 1, 2, 3].map(|i| (1.0 - shape) * self.color[i] + shape * value[i]);
            self.alpha = (1.0 - shape) * self.alpha + shape * value_alpha;
            self.group = (1.0 - shape) * self.group + alpha;
        } else {
            self.color = value;
            self.alpha = value_alpha;
            self.group = alpha + self.group * (1.0 - alpha);
        }
        self.shape = shape + self.shape * (1.0 - shape);
    }
}

/// A deterministic generator of test values.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        #[expect(clippy::cast_precision_loss, reason = "53 random bits")]
        let value = (self.0 >> 11) as f64 / (1_u64 << 53) as f64;
        // Exact zeros and ones are the interesting edges.
        match self.0 % 9 {
            0 => 0.0,
            1 => 1.0,
            _ => value,
        }
    }
}

const MODES: [BlendMode; 16] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
];

fn pixel(random: &mut Lcg) -> Old {
    let alpha = random.unit();
    Old {
        color: [0; 4].map(|_| random.unit() * alpha),
        alpha,
        group: random.unit(),
        shape: random.unit(),
    }
}

#[test]
fn the_f64_kernel_is_the_earlier_paint_bit_for_bit() {
    let mut random = Lcg(7);
    for process in [Process::Gray, Process::Rgb, Process::Cmyk] {
        let layout = Layout { process, spots: 0 };
        let n = process.components();
        for mode in MODES {
            for knockout in [false, true] {
                for preserve in [0_u64, 0b0101, 0b1111] {
                    for _ in 0..200 {
                        let before = pixel(&mut random);
                        let initial = Old {
                            group: 0.0,
                            shape: 0.0,
                            ..pixel(&mut random)
                        };
                        let color = [0; 4].map(|_| random.unit());
                        let shape = random.unit();
                        let alpha = shape * random.unit();
                        let keep = [0, 1, 2, 3].map(|i| i < n && (preserve >> i) & 1 == 1);
                        let mut old = before;
                        old.paint(color, alpha, shape, keep, mode, initial, knockout, n);
                        let mut new = vec![0.0; layout.stride()];
                        new[..n].copy_from_slice(&before.color[..n]);
                        new[n..].copy_from_slice(&[before.alpha, before.group, before.shape]);
                        let mut start = vec![0.0; layout.stride()];
                        start[..n].copy_from_slice(&initial.color[..n]);
                        start[n] = initial.alpha;
                        let sources = Sources {
                            colors: Colors::Constant(&color[..n]),
                            alpha: &[alpha],
                            shape: &[shape],
                        };
                        let mode_spec = Mode {
                            layout,
                            blend: mode,
                            knockout,
                            preserve: preserve & layout.mask(),
                        };
                        composite(&mut new, &sources, &start, &mode_spec);
                        for i in 0..n {
                            assert_eq!(
                                new[i].to_bits(),
                                old.color[i].to_bits(),
                                "{mode:?} {process:?} knockout {knockout} channel {i}"
                            );
                        }
                        assert_eq!(new[n].to_bits(), old.alpha.to_bits());
                        assert_eq!(new[n + 1].to_bits(), old.group.to_bits());
                        assert_eq!(new[n + 2].to_bits(), old.shape.to_bits());
                    }
                }
            }
        }
    }
}

#[test]
fn spot_channels_take_normal_for_difference_exclusion_and_nonseparable_modes() {
    let layout = Layout {
        process: Process::Rgb,
        spots: 1,
    };
    let backdrop = [0.2, 0.4, 0.6, 0.5, 1.0, 0.0, 0.0];
    for mode in MODES {
        let mut pixel = backdrop;
        let sources = Sources {
            colors: Colors::Constant(&[0.9, 0.1, 0.3, 0.25]),
            alpha: &[1.0],
            shape: &[1.0],
        };
        let spec = Mode {
            layout,
            blend: mode,
            knockout: false,
            preserve: 0,
        };
        composite(&mut pixel, &sources, &[], &spec);
        let normal_spot = matches!(
            mode,
            BlendMode::Normal
                | BlendMode::Difference
                | BlendMode::Exclusion
                | BlendMode::Hue
                | BlendMode::Saturation
                | BlendMode::Color
                | BlendMode::Luminosity
        );
        let expected = if normal_spot {
            0.25
        } else {
            crate::blend::with_blend(mode, Apply(mode, [0.5; 4], [0.25; 4], 1))[0]
        };
        assert_eq!(pixel[3], expected, "{mode:?}");
    }
}

/// The numerator of a value on the 2^-12 grid over 2^12.
fn twelfths(value: f64) -> i128 {
    let bits = value.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap();
    let mantissa = i128::from(bits & ((1 << 52) - 1)) | if exponent == 0 { 0 } else { 1 << 52 };
    let shift = 1075 - exponent.max(1);
    if mantissa == 0 {
        0
    } else if shift >= 12 {
        mantissa >> (shift - 12)
    } else {
        mantissa << (12 - shift)
    }
}

#[test]
fn normal_stores_stay_within_2_to_the_minus_40_levels_of_exact_arithmetic() {
    // Opacities and colours on a 2^-12 grid keep every exact intermediate
    // representable in i128 with a common denominator.
    let mut random = Lcg(11);
    let layout = Layout {
        process: Process::Rgb,
        spots: 0,
    };
    let grid = |random: &mut Lcg| (random.unit() * 4096.0).floor() / 4096.0;
    for _ in 0..200 {
        let mut pixel = vec![0.0; layout.stride()];
        // Exact value as a numerator over 2^(12 · stores).
        let mut reference = [0_i128; 4];
        // The exact values are numerators over 2^scale.
        let mut scale = 12_u32;
        for _ in 0..8 {
            let color = [grid(&mut random), grid(&mut random), grid(&mut random)];
            let alpha = grid(&mut random);
            let sources = Sources {
                colors: Colors::Constant(&color),
                alpha: &[alpha],
                shape: &[alpha],
            };
            let spec = Mode {
                layout,
                blend: BlendMode::Normal,
                knockout: false,
                preserve: 0,
            };
            composite(&mut pixel, &sources, &[], &spec);
            // x' = (1 - a) x + a c, premultiplied, with c and a on 2^-12.
            let a = twelfths(alpha);
            for (i, value) in reference.iter_mut().enumerate() {
                let c = if i < 3 { twelfths(color[i]) } else { 4096 };
                *value = (4096 - a) * *value + a * (c << (scale - 12));
            }
            scale += 12;
        }
        for (i, &value) in reference.iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "a bounded check value")]
            let exact = value as f64 / (2.0_f64).powi(i32::try_from(scale).unwrap());
            let error = (pixel[i] - exact).abs() * 255.0;
            assert!(error <= 8.0 * 2.0_f64.powi(-40), "channel {i}: {error}");
        }
    }
}

/// The exact level after Normal stores of `(opacity, target level)`.
fn reference(start: f64, stores: &[(f64, f64)]) -> f64 {
    stores
        .iter()
        .fold(start, |x, &(a, t)| (1.0 - a) * x + a * t)
}

#[test]
fn n16_stays_within_one_level_for_any_number_of_opaque_stores() {
    let mut random = Lcg(23);
    let mut worst: f64 = 0.0;
    for _ in 0..300 {
        let background = (random.unit() * 255.0).round();
        let mut words = [[(background as u16) * 257; 4]];
        let mut stores = Vec::new();
        for _ in 0..(1 + (random.unit() * 400.0) as usize) {
            let color = random.unit();
            let coverage = (1.0 + random.unit() * 254.0).round() as u8;
            let source = [word(color), word(color), word(color), u16::MAX];
            store_n16(&mut words, source, Some(&[coverage]));
            stores.push((f64::from(coverage) / 255.0, color * 255.0));
        }
        // One dynamic store fits the room N16 leaves.
        let (color, alpha) = (random.unit(), random.unit());
        dynamic_n16(&mut words, &Colors::Constant(&[color, color, color]), &[alpha]);
        stores.push((alpha, color * 255.0));
        let exact = reference(background, &stores);
        let byte = f64::from((u32::from(words[0][0]) + 128) / 257);
        worst = worst.max((byte - exact).abs());
    }
    assert!(worst < 0.997, "{worst}");
}

#[test]
fn f32_stays_within_one_level_inside_its_count_bound() {
    let mut random = Lcg(31);
    let mut worst: f64 = 0.0;
    for _ in 0..100 {
        let background = (random.unit() * 255.0).round();
        let mut levels = [[background as f32; 4]];
        let mut stores = Vec::new();
        // 30,000 stores of opacity at least 1/255 · 1/8: the bound's
        // min(N, 1/θ + K) is 2040, far inside 2^-17 · 2040 < 1/2.
        for _ in 0..30_000 {
            let color = random.unit();
            let alpha = (1.0 + (random.unit() * 254.0).floor()) / 255.0 * (0.125 + 0.875 * random.unit());
            store_f32(&mut levels, &Colors::Constant(&[color, color, color]), &[alpha]);
            stores.push((alpha, color * 255.0));
        }
        let exact = reference(background, &stores);
        let byte = f64::from(levels[0][0].round());
        worst = worst.max((byte - exact).abs());
    }
    assert!(worst < 1.0, "{worst}");
}

#[test]
fn u8_rounds_each_store_once() {
    let mut random = Lcg(37);
    for _ in 0..1000 {
        let background = (random.unit() * 255.0).round() as u8;
        let mut bytes = [[background; 4]];
        let (color, alpha) = (random.unit(), random.unit());
        store_u8(&mut bytes, &Colors::Constant(&[color, color, color]), &[alpha]);
        let a = f64::from(pagura_raster::quantize_unit(alpha)) / 255.0;
        let c = f64::from(pagura_raster::quantize_unit(color));
        let exact = (1.0 - a) * f64::from(background) + a * c;
        assert!((f64::from(bytes[0][0]) - exact).abs() <= 0.5 + 1e-9);
    }
}
