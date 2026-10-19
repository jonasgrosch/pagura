//! Group storage with one proven solid interval (11.3.3).
//!
//! Consecutive identical opaque spans need no second store. All other mutation
//! invalidates the interval; floating-point pixels remain authoritative.
//! An admitted page holds bounded N16/F32 storage instead (ADR 0004).

use super::{Pixel, narrow};
use crate::Tile;
use pagura_core::{Error, Result};

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
    pub(super) fn into_rgba(self) -> Result<Vec<u8>> {
        let Some(narrow::Buffer::U8(pixels)) = self.narrow else {
            return Err(Error::invalid_argument(
                "byte output requires a byte surface",
            ));
        };
        Ok(pixels.into_flattened())
    }
    pub(super) fn bytes(&self) -> usize {
        self.pixels.len() * size_of::<Pixel>()
            + self.narrow.as_ref().map_or(0, narrow::Buffer::bytes)
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
