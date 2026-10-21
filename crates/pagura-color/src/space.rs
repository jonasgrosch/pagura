//! Resolved PDF colour spaces (8.6) without PDF objects: the interpreter
//! reads colour-space arrays and dictionaries and builds these values.

use crate::cie::Calibrated;
use crate::function::Function;
use crate::icc::Profile;
use pagura_core::Name;
use std::sync::Arc;

/// The most colour components any space has: DeviceN's 32 colorants
/// (Annex C, Table C.1).
pub const MAX_COMPONENTS: usize = 32;

/// A colour space after resolution (8.6.4–8.6.6).
///
/// The standard closes the set of families, so the enum is exhaustive.
#[derive(Debug, Clone)]
pub enum ColorSpace {
    /// DeviceGray (8.6.4.2).
    DeviceGray,
    /// DeviceRGB (8.6.4.3).
    DeviceRgb,
    /// DeviceCMYK (8.6.4.4).
    DeviceCmyk,
    /// CalGray, CalRGB or Lab (8.6.5.2–8.6.5.4).
    Calibrated(Arc<Calibrated>),
    /// ICCBased (8.6.5.5).
    Icc(Arc<IccSpace>),
    /// Indexed (8.6.6.3).
    Indexed(Arc<Indexed>),
    /// Separation (8.6.6.4).
    Separation(Arc<Separation>),
    /// DeviceN, including NChannel (8.6.6.5).
    DeviceN(Arc<DeviceN>),
    /// Pattern, with the underlying space of uncoloured patterns (8.6.6.2).
    Pattern(Option<Arc<ColorSpace>>),
}

/// An ICCBased space: the profile when it is supported, and the alternate
/// space used otherwise.
#[derive(Debug, Clone)]
pub struct IccSpace {
    /// The parsed profile; `None` when the alternate space is used.
    pub profile: Option<Arc<Profile>>,
    /// The alternate space (`Alternate`, or the device space of `N`).
    pub alternate: Arc<ColorSpace>,
    /// Number of components, `N`: 1, 3 or 4.
    pub components: u8,
    /// The `Range` of each component.
    pub range: [[f64; 2]; 4],
}

/// An Indexed space.
#[derive(Debug, Clone)]
pub struct Indexed {
    /// The base space.
    pub base: Arc<ColorSpace>,
    /// The highest valid index, `hival`.
    pub high: u8,
    /// `(high + 1) × base components` lookup bytes, padded with zeros when
    /// the table was short (a reported repair).
    pub lookup: Arc<[u8]>,
}

/// The colorant a Separation space names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Colorant {
    /// `All`: every colorant of the output, including spot colorants.
    All,
    /// `None`: no colorant; marks in this space paint nothing.
    None,
    /// A process or spot colorant by name.
    Named(Name),
}

/// A Separation space.
#[derive(Debug, Clone)]
pub struct Separation {
    /// The colorant.
    pub colorant: Colorant,
    /// The alternate space.
    pub alternate: Arc<ColorSpace>,
    /// The tint transform from one tint into the alternate space.
    pub tint: Arc<Function>,
}

/// A DeviceN space.
#[derive(Debug, Clone)]
pub struct DeviceN {
    /// The colorant names, `None` entries included (they paint nothing).
    pub colorants: Vec<Colorant>,
    /// The alternate space.
    pub alternate: Arc<ColorSpace>,
    /// The tint transform from the tints into the alternate space.
    pub tint: Arc<Function>,
}

impl ColorSpace {
    /// The number of colour components a colour in this space has; 0 for a
    /// coloured pattern.
    pub fn components(&self) -> usize {
        match self {
            Self::DeviceGray | Self::Indexed(_) | Self::Separation(_) => 1,
            Self::DeviceRgb => 3,
            Self::DeviceCmyk => 4,
            Self::Calibrated(space) => space.components(),
            Self::Icc(space) => usize::from(space.components),
            Self::DeviceN(space) => space.colorants.len(),
            Self::Pattern(base) => base.as_deref().map_or(0, Self::components),
        }
    }

    /// Whether a mark in this space can change any colorant: false for a
    /// Separation `None` and a DeviceN whose colorants are all `None`.
    pub fn paints(&self) -> bool {
        match self {
            Self::Separation(space) => space.colorant != Colorant::None,
            Self::DeviceN(space) => space.colorants.iter().any(|c| *c != Colorant::None),
            Self::Indexed(space) => space.base.paints(),
            _ => true,
        }
    }

    /// The initial colour of this space (8.6.5–8.6.6): black for the device
    /// and CIE spaces, index 0, and full tint for Separation and DeviceN.
    pub fn initial(&self) -> crate::Components {
        match self {
            Self::DeviceCmyk => crate::Components::from_slice(&[0.0, 0.0, 0.0, 1.0]),
            Self::Separation(_) | Self::DeviceN(_) => {
                crate::Components::filled(1.0, self.components())
            }
            _ => crate::Components::filled(0.0, self.components()),
        }
    }
}
