//! Owned pixel surfaces and their colours.

use crate::quantize::multiply;
use pagura_core::{Error, Limits, Result};

/// A straight (non-premultiplied) 8-bit RGBA colour in the output space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba8 {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha; 255 is opaque.
    pub a: u8,
}

impl Rgba8 {
    /// Opaque white, the default page background.
    pub const WHITE: Self = Self::opaque(255, 255, 255);
    /// Opaque black.
    pub const BLACK: Self = Self::opaque(0, 0, 0);
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    /// Creates a colour.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque colour.
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// Converts one premultiplied output pixel using the surface conversion.
    /// Zero alpha produces zero RGBA; nonzero channels round to the nearest
    /// straight byte and saturate when malformed input exceeds its alpha.
    pub fn from_premultiplied(pixel: [u8; 4]) -> Self {
        let [r, g, b, a] = unpremultiply(pixel);
        Self::new(r, g, b, a)
    }

    /// The premultiplied RGBA8 bytes used by pixel surfaces.
    pub fn premultiplied(self) -> [u8; 4] {
        let scale = |channel| multiply(channel, self.a);
        [scale(self.r), scale(self.g), scale(self.b), self.a]
    }
}

/// A row-major, top-left-origin surface of premultiplied RGBA8 pixels.
#[derive(Clone)]
pub struct Pixmap {
    width: u32,
    height: u32,
    data: Vec<u8>,
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
        let pixel = background.premultiplied();
        // A gray or transparent background is one repeated byte; any other
        // is written a whole pixel at a time rather than byte by byte.
        if pixel.iter().all(|&byte| byte == pixel[0]) {
            data.resize(bytes, pixel[0]);
        } else {
            data.resize(bytes, 0);
            for target in data.as_chunks_mut::<4>().0 {
                *target = pixel;
            }
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Takes an existing, tightly packed premultiplied RGBA8 allocation.
    ///
    /// Components use the same layout as [`Self::data_mut`]. Ownership is
    /// transferred without initialization or a second pixel copy.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for zero dimensions or an incorrect byte
    /// count, and `Limit` when the pixel or decoded-byte budget is exceeded.
    pub fn from_premultiplied(
        width: u32,
        height: u32,
        data: Vec<u8>,
        limits: Limits,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid_argument(
                "surface dimensions must be positive",
            ));
        }
        let pixels = usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&n| n <= limits.pixels)
            .ok_or(Error::limit("surface pixel budget exceeded"))?;
        let bytes = pixels
            .checked_mul(4)
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("surface byte budget exceeded"))?;
        if data.len() != bytes {
            return Err(Error::invalid_argument(
                "surface byte count differs from dimensions",
            ));
        }
        if data.capacity() > limits.decoded_bytes {
            return Err(Error::limit(
                "surface retained allocation exceeds byte budget",
            ));
        }
        Ok(Self {
            width,
            height,
            data,
        })
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

    /// Consumes the surface and converts its existing allocation to straight
    /// RGBA8 in place. Fully transparent pixels become zero RGBA; no pixel
    /// allocation is made. The conversion matches [`Self::to_straight_rgba`].
    pub fn into_straight_rgba(mut self) -> Vec<u8> {
        for pixel in self.data.as_chunks_mut::<4>().0 {
            *pixel = unpremultiply(*pixel);
        }
        self.data
    }

    /// Composites onto `background` and drops alpha, giving packed RGB8.
    pub fn to_rgb_over(&self, background: Rgba8) -> Vec<u8> {
        let under = Rgba8 {
            a: 255,
            ..background
        }
        .premultiplied();
        let (pixels, _) = self.data.as_chunks::<4>();
        pixels
            .iter()
            .flat_map(|pixel| {
                let inverse = 255 - pixel[3];
                [0, 1, 2].map(|i| pixel[i].saturating_add(multiply(under[i], inverse)))
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
    use super::Pixmap;
    use crate::Rgba8;

    #[test]
    fn owned_rgba_transfer_preserves_allocation_and_checks_budgets() {
        let mut data = Vec::with_capacity(32);
        data.extend_from_slice(&[128, 0, 0, 128, 0, 0, 255, 255]);
        let pointer = data.as_ptr();
        let surface =
            Pixmap::from_premultiplied(2, 1, data, pagura_core::Limits::default()).unwrap();
        assert_eq!(surface.data().as_ptr(), pointer);
        assert_eq!(surface.data(), [128, 0, 0, 128, 0, 0, 255, 255]);
        let mut spare = Vec::with_capacity(32);
        spare.resize(8, 0);
        assert!(
            Pixmap::from_premultiplied(
                2,
                1,
                spare,
                pagura_core::Limits {
                    decoded_bytes: 8,
                    ..pagura_core::Limits::default()
                }
            )
            .is_err()
        );
        assert!(Pixmap::from_premultiplied(0, 1, vec![], pagura_core::Limits::default()).is_err());
        assert!(
            Pixmap::from_premultiplied(2, 1, vec![0; 4], pagura_core::Limits::default()).is_err()
        );
        assert!(
            Pixmap::from_premultiplied(
                2,
                1,
                vec![0; 8],
                pagura_core::Limits {
                    decoded_bytes: 7,
                    ..pagura_core::Limits::default()
                }
            )
            .is_err()
        );
        assert!(
            Pixmap::from_premultiplied(
                2,
                1,
                vec![0; 8],
                pagura_core::Limits {
                    pixels: 1,
                    ..pagura_core::Limits::default()
                }
            )
            .is_err()
        );
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

    #[test]
    fn consuming_straight_conversion_reuses_storage_and_normalizes_alpha() {
        let data = vec![64, 32, 0, 128, 9, 10, 11, 0, 17, 23, 42, 255];
        let pointer = data.as_ptr();
        let surface =
            Pixmap::from_premultiplied(3, 1, data, pagura_core::Limits::default()).unwrap();
        let straight = surface.into_straight_rgba();
        assert_eq!(straight.as_ptr(), pointer);
        assert_eq!(straight, [128, 64, 0, 128, 0, 0, 0, 0, 17, 23, 42, 255]);
    }

    #[test]
    fn rows_outside_the_surface_are_none() {
        let pixmap = Pixmap::new(3, 2, Rgba8::BLACK, 6).unwrap();
        assert_eq!(pixmap.row(1), Some(&[0, 0, 0, 255].repeat(3)[..]));
        assert_eq!(pixmap.row(2), None);
        assert_eq!(pixmap.row(u32::MAX), None);
    }
}
