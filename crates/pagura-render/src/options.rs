//! What a rendering asks for: resolution, background, overprint preview,
//! fast parameters and the pixels of the page to produce.

use pagura_core::{Error, Result};
use pagura_paint::{FastParams, OverprintMode};
use pagura_raster::{DeviceRect, Rgba8};

/// Options for rendering a page.
///
/// A plain record built with functional record update
/// (`RenderOptions { fast: FastParams::COMPETITIVE, ..RenderOptions::from_dpi(150.0) }`),
/// so it is not `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Device pixels per default user-space unit (1/72 inch); 1.0 is 72 dpi.
    pub scale: f64,
    /// The colour the page is composited onto.
    pub background: Rgba8,
    /// Whether overprinting is simulated (8.6.7, 11.7.4).
    pub overprint: OverprintPreview,
    /// The fast parameters (ADR 0034). [`FastParams::EXACT`], the default,
    /// renders every pixel within one level of its exact result (ADR 0004);
    /// [`FastParams::COMPETITIVE`] switches all four on.
    pub fast: FastParams,
}

impl Default for RenderOptions {
    /// 72 dpi, exact, on white, with overprint preview [`OverprintPreview::Auto`].
    fn default() -> Self {
        Self {
            scale: 1.0,
            background: Rgba8::WHITE,
            overprint: OverprintPreview::Auto,
            fast: FastParams::EXACT,
        }
    }
}

impl RenderOptions {
    /// The default options at `dpi` dots per inch.
    pub fn from_dpi(dpi: f64) -> Self {
        Self {
            scale: dpi / 72.0,
            ..Self::default()
        }
    }
}

/// The display policy for overprinting (8.6.7, 11.7.4). It selects how a
/// recorded scene is rendered and never changes what is recorded. Without
/// simulation, overprinting marks paint as ordinary composite content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum OverprintPreview {
    /// Simulate only when the document declares a PDF/X print intent: an
    /// output intent with `S /GTS_PDFX`, or a `GTS_PDFXVersion` string in the
    /// document information dictionary (14.11.5). This detects a declaration,
    /// not PDF/X conformance or an output ICC transform.
    #[default]
    Auto,
    /// Simulate wherever overprinting can affect painted colorants.
    Always,
    /// Ignore overprint flags.
    Never,
}

impl OverprintPreview {
    /// The renderer's mode for a scene whose document does or does not
    /// declare a PDF/X print intent.
    pub(crate) fn resolve(self, print_intent: bool) -> OverprintMode {
        match self {
            Self::Always => OverprintMode::Simulate,
            Self::Auto if print_intent => OverprintMode::Simulate,
            Self::Auto | Self::Never => OverprintMode::Ignore,
        }
    }
}

/// A rectangle of device pixels within a rendered page. Tiles of one page
/// rendered with the same options assemble byte for byte into the whole
/// page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    /// Left edge in device pixels.
    pub x: u32,
    /// Top edge in device pixels.
    pub y: u32,
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
}

impl Tile {
    /// The tile as device pixels of a page of `size` pixels.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for an empty tile or one that does not lie
    /// within the page.
    pub(crate) fn region(self, (width, height): (u32, u32)) -> Result<DeviceRect> {
        let right = self.x.checked_add(self.width).filter(|&x| x <= width);
        let bottom = self.y.checked_add(self.height).filter(|&y| y <= height);
        match (right, bottom) {
            (Some(x1), Some(y1)) if self.width > 0 && self.height > 0 => Ok(DeviceRect {
                x0: self.x,
                y0: self.y,
                x1,
                y1,
            }),
            _ => Err(Error::invalid_argument("tile lies outside the page")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;

    #[test]
    fn defaults_are_exact_on_white_at_72_dpi_with_automatic_overprint() {
        let options = RenderOptions::default();
        assert_eq!(options.scale, 1.0);
        assert_eq!(options.background, Rgba8::WHITE);
        assert_eq!(options.overprint, OverprintPreview::Auto);
        assert!(options.fast.is_exact());
        assert_eq!(RenderOptions::from_dpi(144.0).scale, 2.0);
        assert_eq!(
            RenderOptions::from_dpi(150.0),
            RenderOptions {
                scale: 150.0 / 72.0,
                ..RenderOptions::default()
            }
        );
    }

    #[test]
    fn overprint_preview_resolves_with_the_print_intent() {
        for (preview, intent, expected) in [
            (OverprintPreview::Auto, true, OverprintMode::Simulate),
            (OverprintPreview::Auto, false, OverprintMode::Ignore),
            (OverprintPreview::Always, false, OverprintMode::Simulate),
            (OverprintPreview::Always, true, OverprintMode::Simulate),
            (OverprintPreview::Never, true, OverprintMode::Ignore),
            (OverprintPreview::Never, false, OverprintMode::Ignore),
        ] {
            assert_eq!(preview.resolve(intent), expected, "{preview:?} {intent}");
        }
    }

    #[test]
    fn tiles_must_be_nonempty_and_inside_the_page() {
        let tile = |x, y, width, height| Tile {
            x,
            y,
            width,
            height,
        };
        assert_eq!(
            tile(2, 3, 4, 5).region((6, 8)).unwrap(),
            DeviceRect {
                x0: 2,
                y0: 3,
                x1: 6,
                y1: 8
            }
        );
        for outside in [
            tile(0, 0, 0, 1),
            tile(0, 0, 1, 0),
            tile(3, 0, 4, 1),
            tile(0, 8, 1, 1),
            tile(u32::MAX, 0, 2, 1),
            tile(0, u32::MAX, 1, 2),
        ] {
            assert_eq!(
                outside.region((6, 8)).unwrap_err().kind,
                ErrorKind::InvalidArgument,
                "{outside:?}"
            );
        }
    }
}
