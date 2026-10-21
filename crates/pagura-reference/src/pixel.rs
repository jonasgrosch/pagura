//! The basic compositing formula of clause 11 for one pixel, in f64.
//!
//! A pixel holds premultiplied colour, alpha, the group alpha (the alpha
//! of the group's own elements without its backdrop, 11.4.4) and shape
//! (11.3.7). A source is a straight colour with alpha and shape, already
//! scaled by coverage, constant alpha and soft mask.

use crate::blend::{blend, spot_mode};
use crate::color::{Channels, MAX};
use pagura_color::Process;
use pagura_interpret::{BlendMode, Composite};

/// One pixel of a compositing buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pixel {
    /// Premultiplied colour.
    pub(crate) color: Channels,
    /// Alpha.
    pub(crate) alpha: f64,
    /// Group alpha.
    pub(crate) group: f64,
    /// Shape.
    pub(crate) shape: f64,
}

impl Pixel {
    /// Transparent black.
    pub(crate) const TRANSPARENT: Self = Self {
        color: [0.0; MAX],
        alpha: 0.0,
        group: 0.0,
        shape: 0.0,
    };

    /// An opaque colour, as an initial backdrop.
    pub(crate) fn opaque(color: &[f64]) -> Self {
        let mut pixel = Self::TRANSPARENT;
        pixel.color[..color.len()].copy_from_slice(color);
        pixel.alpha = 1.0;
        pixel
    }

    /// A group's colour at this pixel with its initial backdrop `base`
    /// removed (11.4.4), clipped; black where its group alpha is zero.
    pub(crate) fn group_color(&self, base: &Self, channels: usize) -> Channels {
        let mut out = [0.0; MAX];
        if self.group > 0.0 {
            for ((out, &c), &b) in out
                .iter_mut()
                .zip(&self.color)
                .zip(&base.color)
                .take(channels)
            {
                *out = ((c - (1.0 - self.group) * b) / self.group).clamp(0.0, 1.0);
            }
        }
        out
    }

    /// The straight colour, clipped; black where alpha is zero.
    pub(crate) fn straight(&self, channels: usize) -> Channels {
        let mut out = [0.0; MAX];
        if self.alpha > 0.0 {
            for (out, &c) in out.iter_mut().zip(&self.color).take(channels) {
                *out = (c / self.alpha).clamp(0.0, 1.0);
            }
        }
        out
    }
}

/// What one element contributes to one pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Source {
    /// Straight colour.
    pub(crate) color: Channels,
    /// Alpha.
    pub(crate) alpha: f64,
    /// Shape.
    pub(crate) shape: f64,
    /// Channels the element leaves to the backdrop (overprint, 11.7.4.3):
    /// bit `i` for channel `i`.
    pub(crate) preserve: u64,
}

impl Source {
    /// Applies the constant alpha and soft-mask value of `state`; with
    /// alpha-is-shape they scale shape too (11.6.4.3).
    pub(crate) fn apply(&mut self, state: &Composite, mask: f64) {
        let factor = state.alpha * mask;
        self.alpha *= factor;
        if state.alpha_is_shape {
            self.shape *= factor;
        }
    }
}

/// How the elements of a group composite with each other (11.4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Elements {
    /// Each element composites with the result of the earlier ones.
    Accumulate,
    /// Each element composites with the group's initial backdrop, replacing
    /// earlier elements by its shape.
    Knockout,
}

/// The blending space and mode of one compositing step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Blending {
    /// The blend mode.
    pub(crate) mode: BlendMode,
    /// The blending space's process components, which come first.
    pub(crate) process: Process,
    /// Process and spot channels.
    pub(crate) channels: usize,
}

impl Pixel {
    /// Composites `source` into the pixel (11.3.6 and, for knockout,
    /// 11.4.6), with `initial` the group's initial backdrop at this pixel.
    pub(crate) fn paint(
        &mut self,
        source: &Source,
        blending: Blending,
        initial: &Self,
        elements: Elements,
    ) {
        let n = blending.channels;
        let knockout = elements == Elements::Knockout;
        let base = if knockout { *initial } else { *self };
        let opacity = if knockout && source.shape > 0.0 {
            source.alpha / source.shape
        } else {
            source.alpha
        };
        let preserved = |i: usize| i < 64 && source.preserve & (1 << i) != 0;
        let back = base.straight(n);
        let mut color = source.color;
        let mut mixed = [0.0; MAX];
        // Overprinting substitutes backdrop components: under Normal into
        // the result, under another mode into the source of an implicit
        // non-isolated group (11.7.4.3).
        for i in 0..n {
            if preserved(i) && mode_of(blending, i) != BlendMode::Normal {
                color[i] = (1.0 - base.alpha) * color[i] + base.alpha * back[i];
            }
        }
        blend(
            blending.mode,
            blending.process,
            n,
            (&back, &color),
            &mut mixed,
        );
        for i in 0..n {
            if preserved(i) && mode_of(blending, i) == BlendMode::Normal {
                mixed[i] = back[i];
            }
        }
        let mut value = [0.0; MAX];
        for i in 0..n {
            value[i] = (1.0 - opacity) * base.color[i]
                + opacity * ((1.0 - base.alpha) * color[i] + base.alpha * mixed[i]);
        }
        let value_alpha = opacity + base.alpha * (1.0 - opacity);
        let shape = source.shape;
        if knockout {
            for (c, v) in self.color.iter_mut().zip(&value).take(n) {
                *c = (1.0 - shape) * *c + shape * v;
            }
            self.alpha = (1.0 - shape) * self.alpha + shape * value_alpha;
            self.group = (1.0 - shape) * self.group + source.alpha;
        } else {
            self.color[..n].copy_from_slice(&value[..n]);
            self.alpha = value_alpha;
            self.group = source.alpha + self.group * (1.0 - source.alpha);
        }
        self.shape = shape + self.shape * (1.0 - shape);
    }
}

/// The mode channel `i` blends with.
fn mode_of(blending: Blending, i: usize) -> BlendMode {
    if i < blending.process.components() {
        blending.mode
    } else {
        spot_mode(blending.mode)
    }
}

#[cfg(test)]
mod tests;
