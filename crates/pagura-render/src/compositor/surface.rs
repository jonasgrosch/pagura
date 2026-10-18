//! Group surfaces and the per-pixel compositing arithmetic (11.3, 11.4).
//!
//! A surface remembers one proven solid interval: consecutive identical
//! opaque spans need no second store. All other mutation invalidates the
//! interval; floating-point pixels remain authoritative. An admitted page
//! holds bounded N16/F32 storage instead (ADR 0004).

use super::{Model, narrow};
use crate::Tile;
use crate::transparency::Composite;
use pagura_core::{Error, Result};

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Pixel {
    pub(super) color: [f64; 4],
    pub(super) alpha: f64,
    pub(super) group: f64,
    pub(super) shape: f64,
}
impl Pixel {
    pub(super) fn source(color: [f64; 4], alpha: f64) -> Self {
        Self {
            color: color.map(|v| v * alpha),
            alpha,
            ..Self::default()
        }
    }
    pub(super) fn straight(self) -> [f64; 4] {
        if self.alpha > 0.0 {
            self.color.map(|v| (v / self.alpha).clamp(0.0, 1.0))
        } else {
            [0.0; 4]
        }
    }
    pub(super) fn paint(
        &mut self,
        source: Source,
        state: &Composite,
        initial: Self,
        knockout: bool,
        model: Model,
    ) {
        let Source {
            color,
            alpha,
            shape,
            preserve,
            spot,
        } = source;
        let base = if knockout { initial } else { *self };
        let opacity = if knockout && shape > 0.0 {
            alpha / shape
        } else {
            alpha
        };
        // Compatible overprinting substitutes backdrop components (11.7.4.3).
        // For another blend mode, this is the source of the implicit
        // non-isolated group, before applying the requested outer blend.
        let back = if preserve.iter().any(|&v| v) {
            base.straight()
        } else {
            [0.0; 4]
        };
        let blend_mode = if spot
            && matches!(
                state.blend,
                crate::blend::BlendMode::Difference
                    | crate::blend::BlendMode::Exclusion
                    | crate::blend::BlendMode::Hue
                    | crate::blend::BlendMode::Saturation
                    | crate::blend::BlendMode::Color
                    | crate::blend::BlendMode::Luminosity
            ) {
            crate::blend::BlendMode::Normal
        } else {
            state.blend
        };
        let (color, blend) = if blend_mode == crate::blend::BlendMode::Normal {
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
            (color, blend_mode.apply(base.straight(), color, model))
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
#[derive(Debug, Clone, Copy)]
pub(super) struct Source {
    pub(super) color: [f64; 4],
    pub(super) alpha: f64,
    pub(super) shape: f64,
    pub(super) preserve: [bool; 4],
    pub(super) spot: bool,
}
impl Source {
    pub(super) fn new(color: [f64; 4], alpha: f64, shape: f64) -> Self {
        Self {
            color,
            alpha,
            shape,
            preserve: [false; 4],
            spot: false,
        }
    }
    pub(super) fn covered(mut self, coverage: u8) -> Self {
        let factor = f64::from(coverage) / 255.0;
        self.alpha *= factor;
        self.shape *= factor;
        self
    }
    pub(super) fn apply(mut self, state: &Composite, mask: f64) -> Self {
        let factor = state.alpha * mask;
        self.alpha *= factor;
        if state.alpha_is_shape {
            self.shape *= factor;
        }
        self
    }
}

pub(super) struct Surface {
    pub(super) tile: Tile,
    pixels: Vec<Pixel>,
    /// The page's bounded storage; `pixels` is then empty.
    narrow: Option<narrow::Buffer>,
    solid: Option<SolidSpan>,
}

#[derive(Clone, Copy)]
struct SolidSpan {
    start: usize,
    end: usize,
    color: [u64; 4],
}

impl Surface {
    pub(super) fn new(tile: Tile, pixels: Vec<Pixel>) -> Self {
        Self {
            tile,
            pixels,
            narrow: None,
            solid: None,
        }
    }
    pub(super) fn with_buffer(tile: Tile, buffer: narrow::Buffer) -> Self {
        Self {
            tile,
            pixels: Vec::new(),
            narrow: Some(buffer),
            solid: None,
        }
    }
    pub(super) fn narrow(&self) -> Option<&narrow::Buffer> {
        self.narrow.as_ref()
    }
    /// Every store into bounded storage also ends the solid interval.
    pub(super) fn narrow_mut(&mut self) -> Option<&mut narrow::Buffer> {
        self.solid = None;
        self.narrow.as_mut()
    }
    pub(super) fn pixels(&self) -> &[Pixel] {
        &self.pixels
    }
    pub(super) fn pixels_mut(&mut self) -> &mut [Pixel] {
        self.solid = None;
        &mut self.pixels
    }

    pub(super) fn pixel(&self, x: u32, y: u32) -> Pixel {
        self.index(x, y)
            .and_then(|i| self.pixels.get(i))
            .copied()
            .unwrap_or_default()
    }
    pub(super) fn index(&self, x: u32, y: u32) -> Option<usize> {
        let (x, y) = (x.checked_sub(self.tile.x)?, y.checked_sub(self.tile.y)?);
        if x >= self.tile.width || y >= self.tile.height {
            return None;
        }
        usize::try_from(u64::from(y) * u64::from(self.tile.width) + u64::from(x)).ok()
    }
    /// Index of the first of `len` pixels from (x, y), when all lie in one tile row.
    pub(super) fn span(&self, x: u32, y: u32, len: usize) -> Option<usize> {
        let last = x.checked_add(u32::try_from(len.checked_sub(1)?).ok()?)?;
        self.index(last, y)?;
        self.index(x, y)
    }
    pub(super) fn fill_solid(
        &mut self,
        start: usize,
        count: usize,
        color: [f64; 4],
    ) -> Result<bool> {
        let pixels = self
            .pixels
            .get_mut(start..start + count)
            .ok_or(Error::invalid_argument("mask outside group"))?;
        let end = start + count;
        let color_bits = color.map(f64::to_bits);
        if self
            .solid
            .is_some_and(|s| s.color == color_bits && start >= s.start && end <= s.end)
        {
            return Ok(true);
        }
        // The Normal formula returns the source exactly at zero or unit backdrop
        // alpha (11.3.3). Other alphas take the general f64 evaluation.
        if !pixels.iter().all(|p| p.alpha == 0.0 || p.alpha == 1.0) {
            return Ok(false);
        }
        pixels.fill(Pixel {
            color,
            alpha: 1.0,
            group: 1.0,
            shape: 1.0,
        });
        self.solid = Some(match self.solid {
            Some(s) if s.color == color_bits && start <= s.end && end >= s.start => SolidSpan {
                start: start.min(s.start),
                end: end.max(s.end),
                color: color_bits,
            },
            _ => SolidSpan {
                start,
                end,
                color: color_bits,
            },
        });
        Ok(true)
    }
    pub(super) fn bytes(&self) -> usize {
        self.pixels.len() * size_of::<Pixel>()
            + self.narrow.as_ref().map_or(0, narrow::Buffer::bytes)
    }
}
#[derive(Clone, Copy)]
pub(super) enum Backdrop<'a> {
    Transparent,
    Color([f64; 4]),
    Surface(&'a Surface),
}
impl Backdrop<'_> {
    pub(super) fn pixel(self, x: u32, y: u32) -> Pixel {
        let mut p = match self {
            Self::Transparent => Pixel::default(),
            Self::Color(c) => Pixel::source(c, 1.0),
            Self::Surface(s) => s.pixel(x, y),
        };
        p.group = 0.0;
        p.shape = 0.0;
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_solid_intervals_survive_overlap_and_general_mutation() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 8,
            height: 1,
        };
        let mut surface = Surface::new(tile, vec![Pixel::default(); 8]);
        let color = [0.25, 0.5, 0.75, 0.0];
        for (start, count) in [(2, 2), (4, 4), (1, 2), (2, 5)] {
            assert!(surface.fill_solid(start, count, color).unwrap());
        }
        assert_eq!(surface.pixel(0, 0).alpha, 0.0);
        for x in 1..8 {
            let pixel = surface.pixel(x, 0);
            assert_eq!(pixel.color, color);
            assert_eq!([pixel.alpha, pixel.group, pixel.shape], [1.0; 3]);
        }
        assert!(surface.fill_solid(3, 2, [0.5; 4]).unwrap());
        assert!(surface.fill_solid(1, 7, color).unwrap());
        assert!(surface.pixels()[1..].iter().all(|p| p.color == color));
        surface.pixels_mut()[4].alpha = 0.25;
        assert!(!surface.fill_solid(1, 7, color).unwrap());
        assert_eq!(surface.pixel(4, 0).alpha, 0.25);
        assert!(surface.fill_solid(8, 1, color).is_err());
    }

    #[test]
    fn bounded_pages_hold_no_solid_interval() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let buffer = narrow::Buffer::new(super::super::plan::Format::N16, 4, [255; 4]).unwrap();
        let mut surface = Surface::with_buffer(tile, buffer);
        assert_eq!(surface.bytes(), 32);
        // Solid intervals describe f64 pixels only; bounded stores go
        // through `narrow_mut`, which also ends any interval.
        assert!(surface.fill_solid(0, 2, [0.5; 4]).is_err());
        assert!(surface.narrow_mut().is_some());
        assert!(surface.pixels().is_empty());
    }

    #[test]
    fn solid_intervals_preserve_source_channel_bits() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut surface = Surface::new(tile, vec![Pixel::default()]);
        for zero in [0.0_f64, -0.0, 0.0] {
            assert!(surface.fill_solid(0, 1, [zero; 4]).unwrap());
            assert_eq!(
                surface.pixel(0, 0).color.map(f64::to_bits),
                [zero.to_bits(); 4]
            );
        }
    }
}
