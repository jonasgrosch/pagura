//! Prepared colour conversion into a blending space (10.4, 11.3.4,
//! 11.7.4): built once per colour space, target and conversion, then applied
//! without walking the space's structure again.
//!
//! A conversion follows the source space's chain to the target: an Indexed
//! lookup, the range clipping of an ICCBased space without a usable profile,
//! a Separation or DeviceN tint transform, and finally either the target's
//! own device family (clipped, no colour transform) or encoded sRGB (device
//! formulas, CIE colorimetry or an ICC profile) encoded into the target's
//! family, with black generation and undercolour removal for CMYK (10.3.4).
//!
//! Under overprint simulation ([`Colorants::Separated`]) a Separation or
//! DeviceN space whose colorants the target has paints those channels
//! directly, `All` paints every channel, and [`Painted`] tells the channels
//! a colour specifies (11.7.4.3).

mod plan;

use crate::function::Function;
use crate::icc::Intent;
use crate::space::ColorSpace;
use pagura_core::{Error, Meter, Name, Result};
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

/// How a target receives named colorants (8.6.7, 11.7.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Colorants {
    /// Composite colour: every space converts through its alternate and
    /// every colour paints every channel.
    #[default]
    Composite,
    /// Overprint simulation: a Separation or DeviceN space paints the
    /// process channels its process colorant names select (in a CMYK target)
    /// and the spot channels its spot names select, `All` paints every
    /// channel, and [`Painted`] reports the channels a colour specifies. A
    /// space naming a process colorant converts through its alternate in a
    /// Gray or RGB target.
    Separated,
    /// Composite colour with process overprinting simulated: as
    /// `Composite`, except that `OPM 1` zero selection (8.6.7) applies to a
    /// colour natively in DeviceCMYK, as in a soft mask's DeviceCMYK content
    /// while overprint is simulated.
    ProcessOverprint,
}

/// Where converted colours go: a process space plus the spot colorants that
/// overprint simulation keeps as channels of their own (11.7.4).
///
/// Build it with [`Target::new`] or [`Target::separated`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Target {
    /// The process components.
    pub process: Process,
    /// Spot colorant channels after the process components, in order.
    pub spots: Arc<[Name]>,
    /// How named colorants reach the channels.
    pub colorants: Colorants,
}

impl Target {
    /// A composite target of `process` components and no spot channels.
    pub fn new(process: Process) -> Self {
        Self {
            process,
            spots: Arc::new([]),
            colorants: Colorants::Composite,
        }
    }

    /// An overprint-simulating target with `spots` channels after the
    /// process components.
    pub fn separated(process: Process, spots: Arc<[Name]>) -> Self {
        Self {
            process,
            spots,
            colorants: Colorants::Separated,
        }
    }

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

impl Painted {
    /// Every one of `channels` channels.
    pub fn all(channels: usize) -> Self {
        Self(match u32::try_from(channels) {
            Ok(n) if n < u64::BITS => (1 << n) - 1,
            _ => u64::MAX,
        })
    }

    /// Whether channel `channel` is painted.
    pub fn contains(self, channel: usize) -> bool {
        u32::try_from(channel).is_ok_and(|c| c < u64::BITS && self.0 & (1 << c) != 0)
    }
}

/// The most channels a [`Target`] may have: one bit each in [`Painted`].
pub const MAX_CHANNELS: usize = 64;

/// A conversion from one colour space into a [`Target`], prepared once.
#[derive(Debug, Clone)]
pub struct Converter {
    target: Target,
    plan: plan::Plan,
}

impl Converter {
    /// Prepares the conversion of colours in `space` into `target`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a Pattern space (it has no
    /// components), `Unsupported` for an ICC profile whose transform for the
    /// intent is outside the supported subset, and a `Limit` error for a
    /// target with more than [`MAX_CHANNELS`] channels or a space nested
    /// more than 64 levels deep.
    pub fn new(space: &ColorSpace, target: &Target, conversion: &Conversion) -> Result<Self> {
        Self::new_metered(
            space,
            target,
            conversion,
            &mut Meter::new(usize::MAX, "colour conversion allocation failed"),
        )
    }

    /// Prepares a conversion, reserving every owned allocation against
    /// `bytes` before construction, including overlap while a buffer grows.
    /// Shared profiles, functions and target colorant names are not charged.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], or a `Limit` error when an allocation exceeds
    /// `bytes` or cannot be fulfilled.
    pub fn new_metered(
        space: &ColorSpace,
        target: &Target,
        conversion: &Conversion,
        bytes: &mut Meter,
    ) -> Result<Self> {
        if target.channels() > MAX_CHANNELS {
            return Err(Error::limit("too many colour channels"));
        }
        Ok(Self {
            target: target.clone(),
            plan: plan::Plan::new(space, target, conversion, bytes)?,
        })
    }

    /// Heap bytes owned by this converter, excluding its inline value and
    /// shared profiles, functions and target colorant names.
    pub fn owned_bytes(&self) -> usize {
        self.plan.owned_bytes()
    }

    /// The target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The `function_work` units one conversion costs at most: tint
    /// transforms, and black generation and undercolour removal.
    pub fn cost(&self) -> usize {
        self.plan.cost()
    }

    /// The channels every colour of the space paints, as
    /// [`Self::convert`] returns them.
    pub fn painted(&self) -> Painted {
        self.plan.painted()
    }

    /// Converts `components` (clipped to the space's ranges) into
    /// `output`, which holds [`Target::channels`] values, and returns the
    /// channels the colour paints. Additive channels hold intensity and
    /// subtractive ones `1 − tint`, as compositing stores them; a channel
    /// the colour leaves alone holds 1 (no colorant). A space that paints
    /// nothing (Separation `None`) paints no channel.
    pub fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        self.plan.convert(components, output);
        self.plan.painted()
    }

    /// The channels a colour paints under overprint mode 1 (`OPM 1`,
    /// 8.6.7): as [`Self::painted`], except that a DeviceCMYK colour in a
    /// separated CMYK target (directly, through Indexed or through an
    /// ICCBased space without a usable profile) leaves the process channels
    /// of its zero components to the backdrop.
    pub fn nonzero(&self, components: &[f64]) -> Painted {
        self.plan.nonzero(components)
    }
}

/// The encoding of an sRGB colour into a process space's channels (10.3),
/// as every conversion into a [`Target`] through RGB ends: RGB as it is,
/// gray as its [`luminosity`], and CMYK by black generation and undercolour
/// removal (10.3.4), each channel `1 − tint`.
#[derive(Debug, Clone)]
pub struct Encoder(plan::Encode);

impl Encoder {
    /// The encoding into `process`, with the black generation and
    /// undercolour removal of `conversion` for CMYK.
    pub fn new(process: Process, conversion: &Conversion) -> Self {
        Self(plan::Encode::new(process, conversion))
    }

    /// Writes `rgb` encoded into `output`, which holds the process
    /// channels.
    pub fn encode(&self, rgb: [f64; 3], output: &mut [f64]) {
        self.0.apply(rgb, output);
    }

    /// The `function_work` units one encoding costs: black generation and
    /// undercolour removal.
    pub fn cost(&self) -> usize {
        self.0.cost()
    }
}

pub use plan::luminosity;

#[cfg(test)]
mod tests;
