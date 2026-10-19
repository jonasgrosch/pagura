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
use pagura_raster::Mask;

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

/// A group's pixels over its tile, holding their bytes until dropped.
pub(super) struct Surface {
    pub(super) tile: Tile,
    pixels: Vec<Pixel>,
    /// The page's bounded storage; `pixels` is then empty.
    narrow: Option<narrow::Buffer>,
    solid: Option<SolidSpan>,
    _held: super::memory::Held,
}

#[derive(Clone, Copy)]
struct SolidSpan {
    start: usize,
    end: usize,
    color: [u64; 4],
}

impl Surface {
    /// A surface whose bytes `held` keeps charged to the render's memory.
    pub(super) fn new(tile: Tile, pixels: Vec<Pixel>, held: super::memory::Held) -> Self {
        Self {
            tile,
            pixels,
            narrow: None,
            solid: None,
            _held: held,
        }
    }
    /// Bounded page storage whose bytes `held` keeps charged.
    pub(super) fn with_buffer(
        tile: Tile,
        buffer: narrow::Buffer,
        held: super::memory::Held,
    ) -> Self {
        Self {
            tile,
            pixels: Vec::new(),
            narrow: Some(buffer),
            solid: None,
            _held: held,
        }
    }
    pub(super) fn narrow(&self) -> Option<&narrow::Buffer> {
        self.narrow.as_ref()
    }
    /// Every store into bounded storage also ends the solid interval; an
    /// f64 surface keeps its interval.
    pub(super) fn narrow_mut(&mut self) -> Option<&mut narrow::Buffer> {
        let buffer = self.narrow.as_mut()?;
        self.solid = None;
        Some(buffer)
    }
    /// A surface outside any render's memory.
    #[cfg(test)]
    pub(super) fn detached(tile: Tile, pixels: Vec<Pixel>) -> Self {
        let held = super::memory::Memory::new(0).hold(0, "detached surface");
        Self::new(tile, pixels, held.expect("an empty holding fits any budget"))
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
    pub(super) fn fill_solid(&mut self, start: usize, count: usize, color: [f64; 4]) -> bool {
        let pixels = self.pixels.get_mut(start..start + count);
        debug_assert!(
            pixels.is_some(),
            "solid runs come from this surface's spans"
        );
        let Some(pixels) = pixels else {
            return false;
        };
        let end = start + count;
        let color_bits = color.map(f64::to_bits);
        if self
            .solid
            .is_some_and(|s| s.color == color_bits && start >= s.start && end <= s.end)
        {
            return true;
        }
        // The Normal formula returns the source exactly at zero or unit backdrop
        // alpha (11.3.3). Other alphas take the general f64 evaluation.
        if !pixels.iter().all(|p| p.alpha == 0.0 || p.alpha == 1.0) {
            return false;
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
        true
    }
    /// Composites `source` into the pixel at device `(x, y)` of `frame`'s
    /// group, after the soft mask over this surface and the state's constant
    /// alpha (11.3.7, 11.4).
    pub(super) fn composite(
        &mut self,
        (x, y): (u32, u32),
        source: Source,
        state: &Composite,
        soft: Option<&[f64]>,
        frame: Frame<'_>,
    ) {
        let index = self.index(x, y);
        let mask = index.and_then(|i| soft?.get(i)).copied().unwrap_or(1.0);
        if let Some(buffer) = self.narrow_mut() {
            // Admission keeps every mark that reaches bounded storage Normal
            // source-over in RGB, which only F32 pages take (ADR 0004).
            let stored = index.is_some_and(|i| buffer.paint(i, source.apply(state, mask)).is_ok());
            debug_assert!(stored, "marks reach bounded storage inside an F32 page");
            return;
        }
        let pixel = index.and_then(|i| self.pixels_mut().get_mut(i));
        debug_assert!(pixel.is_some(), "marks are clipped to their group's tile");
        if let Some(pixel) = pixel {
            frame.paint(pixel, source.apply(state, mask), state, (x, y));
        }
    }
    /// Every painted pixel, with its index and device position.
    pub(super) fn painted(&self) -> impl Iterator<Item = (usize, (u32, u32), Pixel)> + '_ {
        points(self.tile)
            .zip(self.pixels.iter().copied())
            .enumerate()
            .filter(|(_, (_, pixel))| pixel.shape > 0.0)
            .map(|(index, (point, pixel))| (index, point, pixel))
    }
    /// Removes the initial backdrop from a non-isolated group's colours, so
    /// they composite into the parent once (11.4.4).
    pub(super) fn remove_backdrop(&mut self, backdrop: Backdrop<'_>) {
        let painted = points(self.tile)
            .zip(self.pixels_mut())
            .filter(|(_, pixel)| pixel.shape > 0.0);
        for ((x, y), pixel) in painted {
            let base = backdrop.pixel(x, y);
            pixel.color = if pixel.group > 0.0 {
                [0, 1, 2, 3].map(|i| {
                    ((pixel.color[i] - (1.0 - pixel.group) * base.color[i]) / pixel.group)
                        .clamp(0.0, 1.0)
                })
            } else {
                [0.0; 4]
            };
        }
    }
}

/// The device positions of a tile, row by row.
fn points(tile: Tile) -> impl Iterator<Item = (u32, u32)> {
    let Tile {
        x,
        y,
        width,
        height,
    } = tile;
    (y..y + height).flat_map(move |y| (x..x + width).map(move |x| (x, y)))
}

/// A row of nonzero mask coverage from device `(x, y)`: all full or all
/// fractional samples.
pub(super) struct Run<'a> {
    pub(super) x: u32,
    pub(super) y: u32,
    end: u32,
    /// The run's row and first column within its mask.
    pub(super) row: usize,
    pub(super) column: usize,
    pub(super) coverage: &'a [u8],
}
impl<'a> Run<'a> {
    /// Each pixel's device column with its coverage.
    pub(super) fn pixels(&self) -> impl Iterator<Item = (u32, u8)> + 'a {
        (self.x..self.end).zip(self.coverage.iter().copied())
    }
}

/// The covered runs of `mask` in device coordinates, row by row.
pub(super) fn runs(mask: &Mask) -> impl Iterator<Item = Result<Run<'_>>> {
    let (ox, oy) = mask.origin();
    mask.spans().map(move |(row, column, coverage)| {
        let offset = |origin: u32, n: usize| u32::try_from(n).ok()?.checked_add(origin);
        let run = offset(oy, row).zip(offset(ox, column)).and_then(|(y, x)| {
            let end = offset(x, coverage.len())?;
            Some(Run {
                x,
                y,
                end,
                row,
                column,
                coverage,
            })
        });
        run.ok_or(Error::limit("mask run exceeds device coordinates"))
    })
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

/// The group a mark composites into (11.4): the initial backdrop that
/// knockout members read (11.4.6), the group's blending model, whether its
/// members knock each other out, and the nesting depth.
#[derive(Clone, Copy)]
pub(super) struct Frame<'b> {
    pub(super) initial: Backdrop<'b>,
    pub(super) model: Model,
    pub(super) knockout: bool,
    pub(super) depth: usize,
}
impl<'b> Frame<'b> {
    /// The frame of a group's members, one level deeper.
    pub(super) const fn group(
        initial: Backdrop<'b>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Self {
        Self {
            initial,
            model,
            knockout,
            depth: depth + 1,
        }
    }
    /// Composites `source` into `pixel` at device `(x, y)` of this group
    /// (11.3.6). Knockout members composite with the initial backdrop and
    /// replace prior contributions by shape (11.4.6).
    pub(super) fn paint(
        self,
        pixel: &mut Pixel,
        source: Source,
        state: &Composite,
        (x, y): (u32, u32),
    ) {
        let Source {
            color,
            alpha,
            shape,
            preserve,
            spot,
        } = source;
        let base = if self.knockout {
            self.initial.pixel(x, y)
        } else {
            *pixel
        };
        let opacity = if self.knockout && shape > 0.0 {
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
            (color, blend_mode.apply(base.straight(), color, self.model))
        };
        let value = [0, 1, 2, 3].map(|i| {
            (1.0 - opacity) * base.color[i]
                + opacity * ((1.0 - base.alpha) * color[i] + base.alpha * blend[i])
        });
        let value_alpha = opacity + base.alpha * (1.0 - opacity);
        if self.knockout {
            pixel.color = [0, 1, 2, 3].map(|i| (1.0 - shape) * pixel.color[i] + shape * value[i]);
            pixel.alpha = (1.0 - shape) * pixel.alpha + shape * value_alpha;
            pixel.group = (1.0 - shape) * pixel.group + alpha;
        } else {
            pixel.color = value;
            pixel.alpha = value_alpha;
            pixel.group = alpha + pixel.group * (1.0 - alpha);
        }
        pixel.shape = shape + pixel.shape * (1.0 - shape);
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
        let mut surface = Surface::detached(tile, vec![Pixel::default(); 8]);
        let color = [0.25, 0.5, 0.75, 0.0];
        for (start, count) in [(2, 2), (4, 4), (1, 2), (2, 5)] {
            assert!(surface.fill_solid(start, count, color));
        }
        assert_eq!(surface.pixel(0, 0).alpha, 0.0);
        for x in 1..8 {
            let pixel = surface.pixel(x, 0);
            assert_eq!(pixel.color, color);
            assert_eq!([pixel.alpha, pixel.group, pixel.shape], [1.0; 3]);
        }
        assert!(surface.fill_solid(3, 2, [0.5; 4]));
        assert!(surface.fill_solid(1, 7, color));
        assert!(surface.pixels()[1..].iter().all(|p| p.color == color));
        surface.pixels_mut()[4].alpha = 0.25;
        assert!(!surface.fill_solid(1, 7, color));
        assert_eq!(surface.pixel(4, 0).alpha, 0.25);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "solid runs come from this surface's spans")]
    fn solid_runs_outside_the_surface_are_an_invariant() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 8,
            height: 1,
        };
        Surface::detached(tile, vec![Pixel::default(); 8]).fill_solid(8, 1, [0.25; 4]);
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
        assert_eq!(buffer.bytes(), 32);
        let memory = super::super::memory::Memory::new(32);
        let held = memory.hold(buffer.bytes(), "bounded page").unwrap();
        let mut surface = Surface::with_buffer(tile, buffer, held);
        assert_eq!(memory.remaining(), 0);
        // Solid intervals describe f64 pixels only; bounded stores go
        // through `narrow_mut`, which also ends any interval.
        assert!(surface.narrow_mut().is_some());
        assert!(surface.pixels().is_empty());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "solid runs come from this surface's spans")]
    fn bounded_pages_take_no_solid_runs() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let buffer = narrow::Buffer::new(super::super::plan::Format::N16, 4, [255; 4]).unwrap();
        let held = super::super::memory::Memory::new(32)
            .hold(buffer.bytes(), "bounded page")
            .unwrap();
        Surface::with_buffer(tile, buffer, held).fill_solid(0, 2, [0.5; 4]);
    }

    #[test]
    fn solid_intervals_preserve_source_channel_bits() {
        let tile = Tile {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut surface = Surface::detached(tile, vec![Pixel::default()]);
        for zero in [0.0_f64, -0.0, 0.0] {
            assert!(surface.fill_solid(0, 1, [zero; 4]));
            assert_eq!(
                surface.pixel(0, 0).color.map(f64::to_bits),
                [zero.to_bits(); 4]
            );
        }
    }
}
