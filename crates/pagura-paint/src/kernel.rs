//! Span kernels (ADR 0034 §4): one span of source pixels composited into a
//! buffer row. The f64 kernel evaluates the transparent imaging model of
//! clause 11 per pixel and is monomorphized over the blend mode, knockout
//! and the colour source; the narrow kernels (ADR 0004) store Normal
//! source-over only. Callers pass per-pixel alpha and shape already reduced
//! by coverage, constant alpha and soft mask.

mod narrow;

pub(crate) use narrow::{
    dynamic_f32, dynamic_n16, f32_bytes, full, level, n16_bytes, overwrite_n16, store_f32,
    store_n16, word,
};

use crate::blend::{Blend, BlendVisitor, with_blend};
use crate::plan::Layout;
use pagura_interpret::BlendMode;

/// The straight colours of a span's pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Colors<'a> {
    /// One colour for every pixel.
    Constant(&'a [f64]),
    /// One colour per pixel, `stride` values apart.
    Strided {
        /// The values.
        values: &'a [f64],
        /// Values from one pixel to the next.
        stride: usize,
    },
}

impl Colors<'_> {
    /// Whether every one of `len` pixels has at least `channels` values.
    pub(crate) fn holds(&self, len: usize, channels: usize) -> bool {
        match *self {
            Self::Constant(color) => color.len() >= channels,
            Self::Strided { values, stride } => {
                stride >= channels
                    && (len == 0
                        || len
                            .checked_sub(1)
                            .and_then(|n| n.checked_mul(stride))
                            .and_then(|n| n.checked_add(channels))
                            .is_some_and(|n| n <= values.len()))
            }
        }
    }
}

/// How sources composite into an f64 buffer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mode {
    /// The buffer's channels.
    pub(crate) layout: Layout,
    /// Blend mode.
    pub(crate) blend: BlendMode,
    /// The buffer is a knockout group's.
    pub(crate) knockout: bool,
    /// Channels that keep the backdrop (overprint, 11.7.4.3).
    pub(crate) preserve: u64,
}

/// One span of sources.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sources<'a> {
    /// Straight colours.
    pub(crate) colors: Colors<'a>,
    /// Source alpha per pixel.
    pub(crate) alpha: &'a [f64],
    /// Source shape per pixel.
    pub(crate) shape: &'a [f64],
}

/// Composites `sources` into `pixels` (f64 pixels of `mode.layout`), with
/// the knockout group's `initial` backdrop pixels when `mode.knockout`.
pub(crate) fn composite(pixels: &mut [f64], sources: &Sources<'_>, initial: &[f64], mode: &Mode) {
    with_blend(
        mode.blend,
        Run {
            pixels,
            sources,
            initial,
            mode,
        },
    );
}

struct Run<'a, 'b> {
    pixels: &'a mut [f64],
    sources: &'a Sources<'b>,
    initial: &'a [f64],
    mode: &'a Mode,
}

impl BlendVisitor for Run<'_, '_> {
    type Output = ();

    fn visit<B: Blend>(self) {
        if self.mode.knockout {
            colors::<B, true>(self);
        } else {
            colors::<B, false>(self);
        }
    }
}

fn colors<B: Blend, const KNOCKOUT: bool>(run: Run<'_, '_>) {
    match run.sources.colors {
        Colors::Constant(color) => pixels::<B, KNOCKOUT>(run, std::iter::repeat(color)),
        Colors::Strided { values, stride } => {
            pixels::<B, KNOCKOUT>(run, values.chunks(stride.max(1)));
        }
    }
}

#[derive(Clone, Copy)]
struct Counts {
    process: usize,
    channels: usize,
    preserve: u64,
}

fn pixels<'c, B: Blend, const KNOCKOUT: bool>(
    run: Run<'_, '_>,
    colors: impl Iterator<Item = &'c [f64]>,
) {
    let Run {
        pixels,
        sources,
        initial,
        mode,
    } = run;
    let layout = mode.layout;
    let stride = layout.stride();
    let counts = Counts {
        process: layout.process.components(),
        channels: layout.channels(),
        preserve: mode.preserve,
    };
    let sources = sources.alpha.iter().zip(sources.shape).zip(colors);
    let targets = pixels.chunks_exact_mut(stride);
    if KNOCKOUT {
        let initial = initial.chunks_exact(stride);
        for ((target, ((&alpha, &shape), color)), initial) in targets.zip(sources).zip(initial) {
            paint::<B, true>(target, color, [alpha, shape], initial, counts);
        }
    } else {
        for (target, ((&alpha, &shape), color)) in targets.zip(sources) {
            paint::<B, false>(target, color, [alpha, shape], &[], counts);
        }
    }
}

/// One source onto one pixel, in a fixed order of operations: reordering
/// them changes output bytes.
///
/// `base` is the pixel itself, or for knockout the group's initial backdrop.
/// With opacity `q` (alpha, or alpha over shape for knockout), the colour is
/// `(1 − q)·b + q·((1 − αb)·c + αb·B(b, c))` on premultiplied `b`
/// (11.3.3); knockout then mixes it with the pixel by shape (11.4.6).
#[inline]
fn paint<B: Blend, const KNOCKOUT: bool>(
    pixel: &mut [f64],
    color: &[f64],
    [alpha, shape]: [f64; 2],
    initial: &[f64],
    counts: Counts,
) {
    let Counts {
        process,
        channels: n,
        preserve,
    } = counts;
    let base_alpha = if KNOCKOUT { initial[n] } else { pixel[n] };
    let opacity = if KNOCKOUT && shape > 0.0 {
        alpha / shape
    } else {
        alpha
    };
    let straight = |v: f64| {
        if base_alpha > 0.0 {
            (v / base_alpha).clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let keeps = |i: usize| i < 64 && (preserve >> i) & 1 == 1;
    let base = |pixel: &[f64], i: usize| if KNOCKOUT { initial[i] } else { pixel[i] };
    // A non-separable mode blends the process components together, from the
    // same straight backdrop and (overprint-mixed) source as below.
    let joint = if B::SEPARABLE {
        [0.0; 4]
    } else {
        let mut back = [0.0; 4];
        let mut source = [0.0; 4];
        for i in 0..process.min(4) {
            back[i] = straight(base(pixel, i));
            source[i] = mixed(color[i], keeps(i), base_alpha, back[i]);
        }
        B::process(&back, &source, process)
    };
    for i in 0..n {
        let b = base(pixel, i);
        let back = straight(b);
        let keep = keeps(i);
        let (c, blended) = if i < process {
            channel::<B>(color[i], keep, base_alpha, back, joint.get(i).copied())
        } else {
            channel::<B::Spot>(color[i], keep, base_alpha, back, None)
        };
        let value = (1.0 - opacity) * b + opacity * ((1.0 - base_alpha) * c + base_alpha * blended);
        pixel[i] = if KNOCKOUT {
            (1.0 - shape) * pixel[i] + shape * value
        } else {
            value
        };
    }
    let value_alpha = opacity + base_alpha * (1.0 - opacity);
    if KNOCKOUT {
        pixel[n] = (1.0 - shape) * pixel[n] + shape * value_alpha;
        pixel[n + 1] = (1.0 - shape) * pixel[n + 1] + alpha;
    } else {
        pixel[n] = value_alpha;
        pixel[n + 1] = alpha + pixel[n + 1] * (1.0 - alpha);
    }
    pixel[n + 2] = shape + pixel[n + 2] * (1.0 - shape);
}

/// A kept component substitutes the backdrop: for a non-Normal mode, as the
/// source of the implicit non-isolated group (11.7.4.3).
#[inline]
fn mixed(source: f64, keep: bool, base_alpha: f64, back: f64) -> f64 {
    if keep {
        (1.0 - base_alpha) * source + base_alpha * back
    } else {
        source
    }
}

/// The source and blend result of one channel under mode `M`; `joint` is
/// the channel's non-separable result.
#[inline]
fn channel<M: Blend>(
    source: f64,
    keep: bool,
    base_alpha: f64,
    back: f64,
    joint: Option<f64>,
) -> (f64, f64) {
    if M::NORMAL {
        (source, if keep { back } else { source })
    } else {
        let c = mixed(source, keep, base_alpha, back);
        let blended = if M::SEPARABLE {
            M::component(back, c)
        } else {
            joint.unwrap_or(c)
        };
        (c, blended)
    }
}

#[cfg(test)]
mod tests;
