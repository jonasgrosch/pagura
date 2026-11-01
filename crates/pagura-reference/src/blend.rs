//! Blend functions (11.3.5, Tables 134–135) on colours in compositing form,
//! where subtractive components are already complemented (11.3.3).
//!
//! Process channels take the blend mode. Spot channels (11.7.4.2) take the
//! separable modes as they are, and Normal in place of Difference,
//! Exclusion and the non-separable modes.

use crate::color::lum;
use pagura_color::Process;
use pagura_interpret::BlendMode;

/// Writes `B(back, source)` for the first `channels` channels into `out`;
/// `process` channels come first.
pub(crate) fn blend(
    mode: BlendMode,
    process: Process,
    channels: usize,
    (back, source): (&[f64], &[f64]),
    out: &mut [f64],
) {
    let count = process.components().min(channels);
    if mode.is_separable() {
        for i in 0..count {
            out[i] = separable(mode, back[i], source[i]);
        }
    } else {
        non_separable(mode, process, (back, source), out);
    }
    let spot = spot_mode(mode);
    for i in count..channels {
        out[i] = separable(spot, back[i], source[i]);
    }
}

/// The mode spot channels blend with.
pub(crate) fn spot_mode(mode: BlendMode) -> BlendMode {
    match mode {
        BlendMode::Difference | BlendMode::Exclusion => BlendMode::Normal,
        mode if !mode.is_separable() => BlendMode::Normal,
        mode => mode,
    }
}

/// A separable blend function of one component (Table 134).
fn separable(mode: BlendMode, b: f64, s: f64) -> f64 {
    match mode {
        BlendMode::Multiply => b * s,
        BlendMode::Screen => b + s - b * s,
        BlendMode::Overlay => hard_light(s, b),
        BlendMode::Darken => b.min(s),
        BlendMode::Lighten => b.max(s),
        // PDF 2.0 states these boundary cases before the division.
        BlendMode::ColorDodge => {
            if b == 0.0 {
                0.0
            } else if b >= 1.0 - s {
                1.0
            } else {
                b / (1.0 - s)
            }
        }
        BlendMode::ColorBurn => {
            if b == 1.0 {
                1.0
            } else if 1.0 - b >= s {
                0.0
            } else {
                1.0 - (1.0 - b) / s
            }
        }
        BlendMode::HardLight => hard_light(b, s),
        BlendMode::SoftLight => {
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
        }
        BlendMode::Difference => (b - s).abs(),
        BlendMode::Exclusion => b + s - 2.0 * b * s,
        _ => s,
    }
}

fn hard_light(b: f64, s: f64) -> f64 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}

/// The non-separable modes (Table 135) on the process channels: gray as
/// an RGB triple, CMYK on its complemented C, M and Y with K taken from the
/// backdrop, or from the source for Luminosity (11.3.5.3).
fn non_separable(
    mode: BlendMode,
    process: Process,
    (back, source): (&[f64], &[f64]),
    out: &mut [f64],
) {
    let rgb = |c: &[f64]| match process {
        Process::Gray => [c[0]; 3],
        Process::Rgb | Process::Cmyk => [c[0], c[1], c[2]],
    };
    let (b, s) = (rgb(back), rgb(source));
    let result = match mode {
        BlendMode::Hue => set_lum(set_sat(s, saturation(b)), lum(b)),
        BlendMode::Saturation => set_lum(set_sat(b, saturation(s)), lum(b)),
        BlendMode::Color => set_lum(s, lum(b)),
        _ => set_lum(b, lum(s)),
    };
    match process {
        Process::Gray => out[0] = lum(result),
        Process::Rgb => out[..3].copy_from_slice(&result),
        Process::Cmyk => {
            out[..3].copy_from_slice(&result);
            out[3] = if mode == BlendMode::Luminosity {
                source[3]
            } else {
                back[3]
            };
        }
    }
}

fn saturation(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_lum(c: [f64; 3], target: f64) -> [f64; 3] {
    let d = target - lum(c);
    let mut c = c.map(|v| v + d);
    let low = c[0].min(c[1]).min(c[2]);
    let high = c[0].max(c[1]).max(c[2]);
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
    order.sort_by(|&a, &b| c[a].total_cmp(&c[b]));
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

    fn one(mode: BlendMode, b: f64, s: f64) -> f64 {
        let mut out = [0.0];
        blend(mode, Process::Gray, 1, (&[b], &[s]), &mut out);
        out[0]
    }

    #[test]
    fn separable_modes_follow_table_134() {
        let (b, s) = (0.25, 0.75);
        for (mode, expected) in [
            (BlendMode::Normal, 0.75),
            (BlendMode::Multiply, 0.1875),
            (BlendMode::Screen, 0.8125),
            (BlendMode::Overlay, 2.0 * 0.25 * 0.75),
            (BlendMode::Darken, 0.25),
            (BlendMode::Lighten, 0.75),
            (BlendMode::ColorDodge, 1.0),
            (BlendMode::ColorBurn, 0.0),
            (BlendMode::HardLight, 1.0 - 2.0 * 0.75 * 0.25),
            (
                BlendMode::SoftLight,
                0.25 + 0.5 * (((16.0 * 0.25 - 12.0) * 0.25 + 4.0) * 0.25 - 0.25),
            ),
            (BlendMode::Difference, 0.5),
            (BlendMode::Exclusion, 1.0 - 2.0 * 0.1875),
        ] {
            assert!((one(mode, b, s) - expected).abs() < 1e-15, "{mode:?}");
        }
        assert_eq!(one(BlendMode::ColorDodge, 0.25, 0.5), 0.5);
        assert_eq!(one(BlendMode::ColorBurn, 0.75, 0.5), 0.5);
        assert_eq!(one(BlendMode::ColorDodge, 0.0, 1.0), 0.0);
        assert_eq!(one(BlendMode::ColorBurn, 1.0, 0.0), 1.0);
    }

    #[test]
    fn non_separable_modes_keep_hue_saturation_and_luminosity_apart() {
        let back = [0.2, 0.4, 0.6];
        let source = [0.9, 0.1, 0.1];
        let mut out = [0.0; 3];
        blend(
            BlendMode::Luminosity,
            Process::Rgb,
            3,
            (&back, &source),
            &mut out,
        );
        assert!((lum(out) - lum(source)).abs() < 1e-12);
        blend(
            BlendMode::Color,
            Process::Rgb,
            3,
            (&back, &source),
            &mut out,
        );
        assert!((lum(out) - lum(back)).abs() < 1e-12);
        // CMYK keeps the backdrop's black, except for Luminosity.
        let mut cmyk = [0.0; 4];
        blend(
            BlendMode::Hue,
            Process::Cmyk,
            4,
            (&[0.2, 0.4, 0.6, 0.3], &[0.9, 0.1, 0.1, 0.8]),
            &mut cmyk,
        );
        assert_eq!(cmyk[3], 0.3);
        blend(
            BlendMode::Luminosity,
            Process::Cmyk,
            4,
            (&[0.2, 0.4, 0.6, 0.3], &[0.9, 0.1, 0.1, 0.8]),
            &mut cmyk,
        );
        assert_eq!(cmyk[3], 0.8);
    }

    #[test]
    fn spot_channels_take_normal_for_difference_and_non_separable_modes() {
        let mut out = [0.0; 2];
        blend(
            BlendMode::Difference,
            Process::Gray,
            2,
            (&[0.25, 0.25], &[0.75, 0.75]),
            &mut out,
        );
        assert_eq!(out, [0.5, 0.75]);
        blend(
            BlendMode::Multiply,
            Process::Gray,
            2,
            (&[0.5, 0.5], &[0.5, 0.5]),
            &mut out,
        );
        assert_eq!(out, [0.25, 0.25]);
    }
}
