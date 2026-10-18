//! Device colours and paint sources.

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
}

/// What a fill or stroke paints with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Paint {
    /// A uniform colour.
    Solid(Rgba8),
}
