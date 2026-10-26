//! Bounded PNG output for pixel buffers, rendered pages and tiles.
//!
//! The existing codec handles Gray, RGB and straight RGBA buffers. Rendered
//! surfaces are premultiplied; [`encode_pixmap`] consumes and converts their
//! allocation in place before using the same encoder. Output is complete-buffer
//! PNG with no ancillary colour/profile metadata, not incremental output.

pub use pagura_codec::flate::Level;
pub use pagura_codec::png::{ColorType, Options, encode};
use pagura_core::{Error, Limits, Result};
use pagura_raster::Pixmap;

/// Consumes a rendered surface and encodes straight RGBA8 PNG (PNG 6.2).
///
/// Unpremultiplication uses the same integer conversion as browser `ImageData`
/// output; fully transparent pixels become zero RGBA. The pixel allocation is
/// reused, without a second full surface. The codec separately bounds input
/// pixels/bytes, compressed output and scratch as described by [`encode`].
/// A page or tile can be rendered with a transparent or opaque background.
///
/// # Errors
/// As [`encode`], including pixel, decoded-byte and output-stream limits.
/// The surface is consumed on success or failure. No incomplete PNG is returned.
///
/// # Examples
/// ```
/// use pagura::{Limits, Pixmap, Rgba8, png};
/// let limits = Limits::default();
/// let surface = Pixmap::new(2, 1, Rgba8::new(255, 0, 0, 128), limits.pixels)?;
/// let bytes = png::encode_pixmap(surface, png::Level::Default, &limits)?;
/// assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
/// # Ok::<(), pagura::Error>(())
/// ```
pub fn encode_pixmap(surface: Pixmap, level: Level, limits: &Limits) -> Result<Vec<u8>> {
    if surface.data().len() > limits.decoded_bytes || surface.data().len() / 4 > limits.pixels {
        return Err(Error::limit(
            "PNG input exceeds pixel or decoded-byte budget",
        ));
    }
    let (width, height) = (surface.width(), surface.height());
    encode(
        &surface.into_straight_rgba(),
        width,
        height,
        &Options {
            color_type: ColorType::Rgba,
            level,
            limits: *limits,
        },
    )
}
