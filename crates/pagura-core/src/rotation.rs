//! Clockwise quarter-turn page rotations shared by document and display geometry.

use crate::{Error, Result};

/// Clockwise page rotation for display (`/Rotate`, Table 31).
///
/// The set is closed: `/Rotate` is a multiple of 90, so these four values are
/// every rotation a page can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// Upright.
    #[default]
    None,
    /// 90° clockwise.
    Clockwise90,
    /// 180°.
    Clockwise180,
    /// 270° clockwise.
    Clockwise270,
}

impl Rotation {
    /// The rotation in degrees, 0–270.
    pub fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 90,
            Self::Clockwise180 => 180,
            Self::Clockwise270 => 270,
        }
    }
}

impl TryFrom<i64> for Rotation {
    type Error = Error;

    /// Interprets a `/Rotate` value, which must be a multiple of 90 and
    /// may be negative or exceed 360.
    fn try_from(degrees: i64) -> Result<Self> {
        match degrees.rem_euclid(360) {
            0 => Ok(Self::None),
            90 => Ok(Self::Clockwise90),
            180 => Ok(Self::Clockwise180),
            270 => Ok(Self::Clockwise270),
            _ => Err(Error::syntax("Rotate must be a multiple of 90")),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rotate_values_are_multiples_of_ninety_in_any_turn() {
        use super::Rotation;
        for (degrees, expected) in [
            (0, Rotation::None),
            (90, Rotation::Clockwise90),
            (-90, Rotation::Clockwise270),
            (450, Rotation::Clockwise90),
            (-180, Rotation::Clockwise180),
            (720, Rotation::None),
        ] {
            assert_eq!(Rotation::try_from(degrees).unwrap(), expected, "{degrees}");
            assert_eq!(
                i64::from(expected.degrees()),
                degrees.rem_euclid(360),
                "{degrees}"
            );
        }
        for degrees in [45, 1, -1, 91, 359] {
            assert!(Rotation::try_from(degrees).is_err(), "{degrees}");
        }
    }
}
