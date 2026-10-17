//! Owned pixel surfaces.

use crate::Rgba8;
use pagura_core::{Error, Limits, Result};

/// A row-major, top-left-origin surface of premultiplied RGBA8 pixels.
#[derive(Clone)]
pub struct Pixmap {
    width: u32,
    height: u32,
    data: Vec<u8>,
    limits: Limits,
}

/// Summarizes instead of dumping pixel data.
impl std::fmt::Debug for Pixmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pixmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Pixmap {
    /// Allocates a surface filled with `background`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a zero dimension and a limit error if
    /// the surface would exceed `max_pixels`.
    pub fn new(width: u32, height: u32, background: Rgba8, max_pixels: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid_argument(
                "surface dimensions must be positive",
            ));
        }
        let pixels = usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&pixels| pixels <= max_pixels)
            .ok_or(Error::limit("surface pixel budget exceeded"))?;
        let bytes = pixels
            .checked_mul(4)
            .ok_or(Error::limit("surface byte size overflows"))?;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| Error::limit("surface allocation failed"))?;
        let pixel = premultiply(background);
        data.extend(std::iter::repeat_n(pixel, pixels).flatten());
        Ok(Self {
            width,
            height,
            data,
            limits: Limits {
                pixels: max_pixels,
                ..Limits::default()
            },
        })
    }

    /// Allocates a surface with explicit raster, geometry and pixel budgets.
    ///
    /// # Errors
    /// As [`Self::new`], using `limits.pixels` as the surface bound.
    pub fn new_with_limits(
        width: u32,
        height: u32,
        background: Rgba8,
        limits: Limits,
    ) -> Result<Self> {
        let mut surface = Self::new(width, height, background, limits.pixels)?;
        surface.limits = limits;
        Ok(surface)
    }

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA8 bytes, `width * height * 4` long.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutable premultiplied RGBA8 bytes for compositing.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The bytes of row `y`, or `None` outside the surface.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        let stride = usize::try_from(self.width).ok()? * 4;
        let start = usize::try_from(y).ok()?.checked_mul(stride)?;
        self.data.get(start..start.checked_add(stride)?)
    }

    /// Converts to straight (non-premultiplied) RGBA8, as the web canvas
    /// `ImageData` expects.
    pub fn to_straight_rgba(&self) -> Vec<u8> {
        let (pixels, _) = self.data.as_chunks::<4>();
        pixels
            .iter()
            .flat_map(|&pixel| unpremultiply(pixel))
            .collect()
    }

    /// Composites onto `background` and drops alpha, giving packed RGB8.
    pub fn to_rgb_over(&self, background: Rgba8) -> Vec<u8> {
        let under = premultiply(Rgba8 {
            a: 255,
            ..background
        });
        let (pixels, _) = self.data.as_chunks::<4>();
        pixels
            .iter()
            .flat_map(|pixel| {
                let inverse = 255 - u16::from(pixel[3]);
                [0, 1, 2].map(|i| {
                    let value = u16::from(pixel[i]) + div255(u16::from(under[i]) * inverse);
                    u8::try_from(value).unwrap_or(u8::MAX)
                })
            })
            .collect()
    }
}

impl PartialEq for Pixmap {
    fn eq(&self, other: &Self) -> bool {
        (self.width, self.height, &self.data) == (other.width, other.height, &other.data)
    }
}
impl Eq for Pixmap {}

/// `round(value / 255)` for `value <= 255 * 255`.
fn div255(value: u16) -> u16 {
    let value = u32::from(value) + 128;
    u16::try_from((value + (value >> 8)) >> 8).unwrap_or(u16::MAX)
}

fn premultiply(color: Rgba8) -> [u8; 4] {
    let alpha = u16::from(color.a);
    let scale = |channel: u8| u8::try_from(div255(u16::from(channel) * alpha)).unwrap_or(u8::MAX);
    [scale(color.r), scale(color.g), scale(color.b), color.a]
}

fn unpremultiply(pixel: [u8; 4]) -> [u8; 4] {
    let alpha = u32::from(pixel[3]);
    if alpha == 0 {
        return [0; 4];
    }
    let scale = |channel: u8| {
        u8::try_from((u32::from(channel) * 255 + alpha / 2) / alpha).unwrap_or(u8::MAX)
    };
    [scale(pixel[0]), scale(pixel[1]), scale(pixel[2]), pixel[3]]
}

#[cfg(test)]
mod tests {
    use super::{Pixmap, div255};
    use crate::Rgba8;

    #[test]
    fn div255_rounds_exactly() {
        for value in 0..=(255_u16 * 255) {
            let expected = (f64::from(value) / 255.0).round();
            assert_eq!(f64::from(div255(value)), expected, "{value}");
        }
    }

    #[test]
    fn budgets_and_conversions() {
        assert!(Pixmap::new(0, 1, Rgba8::WHITE, 10).is_err());
        assert!(Pixmap::new(4, 4, Rgba8::WHITE, 15).is_err());
        let pixmap = Pixmap::new(2, 1, Rgba8::new(255, 0, 0, 128), 2).unwrap();
        assert_eq!(&pixmap.data()[..4], &[128, 0, 0, 128]);
        assert_eq!(&pixmap.to_straight_rgba()[..4], &[255, 0, 0, 128]);
        assert_eq!(&pixmap.to_rgb_over(Rgba8::WHITE)[..3], &[255, 127, 127]);
    }
}
