//! Prepared colour conversion into a blending space (10.4, 11.3.4,
//! 11.7.4): built once per colour space, target and conversion, then applied
//! without walking the space's structure again.

use crate::function::Function;
use crate::icc::Intent;
use crate::space::ColorSpace;
use pagura_core::{Error, Name, Result};
use std::sync::Arc;

/// The process components of a blending space (11.3.4): the group colour
/// spaces a transparency group may have, each reduced to its device family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Process {
    /// One gray component, additive.
    Gray,
    /// Three RGB components, additive; the page's default.
    #[default]
    Rgb,
    /// Four CMYK components, subtractive.
    Cmyk,
}

impl Process {
    /// The number of process components.
    pub fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }
}

/// Where converted colours go: a process space plus the spot colorants that
/// overprint simulation keeps as channels of their own (11.7.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Target {
    /// The process components.
    pub process: Process,
    /// Spot colorant channels after the process components, in order.
    pub spots: Arc<[Name]>,
}

impl Target {
    /// Process plus spot channels.
    pub fn channels(&self) -> usize {
        self.process.components() + self.spots.len()
    }
}

/// Device-dependent conversion parameters of the graphics state (10.3–10.4).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Conversion {
    /// The rendering intent (8.6.5.8).
    pub intent: Intent,
    /// Black generation (`BG`/`BG2`); the default policy when `None`.
    pub black_generation: Option<Arc<Function>>,
    /// Undercolour removal (`UCR`/`UCR2`); the default policy when `None`.
    pub undercolor_removal: Option<Arc<Function>>,
}

/// The channels a converted colour changes, for overprint (11.7.4.3): bit
/// `i` is channel `i` of the [`Target`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Painted(pub u64);

/// The most channels a [`Target`] may have: one bit each in [`Painted`].
pub const MAX_CHANNELS: usize = 64;

/// A conversion from one colour space into a [`Target`], prepared once.
#[derive(Debug, Clone)]
pub struct Converter {
    target: Target,
}

impl Converter {
    /// Prepares the conversion of colours in `space` into `target`.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported` for a conversion the colour management cannot
    /// perform and a `Limit` error for a target with more than
    /// [`MAX_CHANNELS`] channels.
    pub fn new(space: &ColorSpace, target: &Target, conversion: &Conversion) -> Result<Self> {
        let _ = (space, conversion);
        if target.channels() > MAX_CHANNELS {
            return Err(Error::limit("too many colour channels"));
        }
        Err(Error::invalid_argument(
            "prepared colour conversion is not implemented",
        ))
    }

    /// The target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The `function_work` units one conversion costs at most.
    pub fn cost(&self) -> usize {
        0
    }

    /// Converts `components` (clipped to the space's ranges) into
    /// `output`, which holds [`Target::channels`] values, and returns the
    /// channels the colour paints. Additive channels hold intensity and
    /// subtractive ones `1 − tint`, as compositing stores them.
    pub fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        let _ = components;
        output.fill(0.0);
        Painted(0)
    }
}
