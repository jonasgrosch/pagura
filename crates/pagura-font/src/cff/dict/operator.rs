//! CFF DICT operators (TN 5176 Tables 9, 10 and 23, Appendix H; the CFF2
//! additions `vsindex`, `blend` and `vstore`).

/// A DICT operator: one byte, or the escape byte 12 followed by a second
/// byte, held as `0x0c00 | second`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::cff) struct Operator(u16);

/// The first byte of a two-byte operator.
pub(in crate::cff) const ESCAPE: u8 = 12;
/// Two-byte operators `12 x` are held as `0x0c00 + x`.
const ESCAPED: u16 = 0x0c00;

impl Operator {
    pub(in crate::cff) const VERSION: Self = Self(0);
    pub(in crate::cff) const NOTICE: Self = Self(1);
    pub(in crate::cff) const FULL_NAME: Self = Self(2);
    pub(in crate::cff) const FAMILY_NAME: Self = Self(3);
    pub(in crate::cff) const WEIGHT: Self = Self(4);
    pub(in crate::cff) const FONT_BBOX: Self = Self(5);
    pub(in crate::cff) const BLUE_VALUES: Self = Self(6);
    pub(in crate::cff) const OTHER_BLUES: Self = Self(7);
    pub(in crate::cff) const FAMILY_BLUES: Self = Self(8);
    pub(in crate::cff) const FAMILY_OTHER_BLUES: Self = Self(9);
    pub(in crate::cff) const STD_HW: Self = Self(10);
    pub(in crate::cff) const STD_VW: Self = Self(11);
    pub(in crate::cff) const UNIQUE_ID: Self = Self(13);
    pub(in crate::cff) const XUID: Self = Self(14);
    pub(in crate::cff) const CHARSET: Self = Self(15);
    pub(in crate::cff) const ENCODING: Self = Self(16);
    pub(in crate::cff) const CHAR_STRINGS: Self = Self(17);
    pub(in crate::cff) const PRIVATE: Self = Self(18);
    pub(in crate::cff) const SUBRS: Self = Self(19);
    pub(in crate::cff) const DEFAULT_WIDTH_X: Self = Self(20);
    pub(in crate::cff) const NOMINAL_WIDTH_X: Self = Self(21);
    /// CFF2 only; reserved in CFF.
    pub(in crate::cff) const VSINDEX: Self = Self(22);
    /// CFF2 only; reserved in CFF.
    pub(in crate::cff) const BLEND: Self = Self(23);
    /// CFF2 only; reserved in CFF.
    pub(in crate::cff) const VSTORE: Self = Self(24);
    pub(in crate::cff) const COPYRIGHT: Self = Self(ESCAPED);
    pub(in crate::cff) const IS_FIXED_PITCH: Self = Self(ESCAPED + 1);
    pub(in crate::cff) const ITALIC_ANGLE: Self = Self(ESCAPED + 2);
    pub(in crate::cff) const UNDERLINE_POSITION: Self = Self(ESCAPED + 3);
    pub(in crate::cff) const UNDERLINE_THICKNESS: Self = Self(ESCAPED + 4);
    pub(in crate::cff) const PAINT_TYPE: Self = Self(ESCAPED + 5);
    pub(in crate::cff) const CHARSTRING_TYPE: Self = Self(ESCAPED + 6);
    pub(in crate::cff) const FONT_MATRIX: Self = Self(ESCAPED + 7);
    pub(in crate::cff) const STROKE_WIDTH: Self = Self(ESCAPED + 8);
    pub(in crate::cff) const BLUE_SCALE: Self = Self(ESCAPED + 9);
    pub(in crate::cff) const BLUE_SHIFT: Self = Self(ESCAPED + 10);
    pub(in crate::cff) const BLUE_FUZZ: Self = Self(ESCAPED + 11);
    pub(in crate::cff) const STEM_SNAP_H: Self = Self(ESCAPED + 12);
    pub(in crate::cff) const STEM_SNAP_V: Self = Self(ESCAPED + 13);
    pub(in crate::cff) const FORCE_BOLD: Self = Self(ESCAPED + 14);
    pub(in crate::cff) const LANGUAGE_GROUP: Self = Self(ESCAPED + 17);
    pub(in crate::cff) const EXPANSION_FACTOR: Self = Self(ESCAPED + 18);
    pub(in crate::cff) const INITIAL_RANDOM_SEED: Self = Self(ESCAPED + 19);
    pub(in crate::cff) const SYNTHETIC_BASE: Self = Self(ESCAPED + 20);
    pub(in crate::cff) const POST_SCRIPT: Self = Self(ESCAPED + 21);
    pub(in crate::cff) const BASE_FONT_NAME: Self = Self(ESCAPED + 22);
    pub(in crate::cff) const BASE_FONT_BLEND: Self = Self(ESCAPED + 23);
    pub(in crate::cff) const ROS: Self = Self(ESCAPED + 30);
    pub(in crate::cff) const CID_FONT_VERSION: Self = Self(ESCAPED + 31);
    pub(in crate::cff) const CID_FONT_REVISION: Self = Self(ESCAPED + 32);
    pub(in crate::cff) const CID_FONT_TYPE: Self = Self(ESCAPED + 33);
    pub(in crate::cff) const CID_COUNT: Self = Self(ESCAPED + 34);
    pub(in crate::cff) const UID_BASE: Self = Self(ESCAPED + 35);
    pub(in crate::cff) const FD_ARRAY: Self = Self(ESCAPED + 36);
    pub(in crate::cff) const FD_SELECT: Self = Self(ESCAPED + 37);
    pub(in crate::cff) const FONT_NAME: Self = Self(ESCAPED + 38);

    pub(in crate::cff) fn one_byte(byte: u8) -> Self {
        Self(u16::from(byte))
    }

    pub(in crate::cff) fn escaped(byte: u8) -> Self {
        Self(ESCAPED | u16::from(byte))
    }

    /// Operator slots that TN 5176 Appendix H leaves without semantics in
    /// CFF: 22-27, escaped 15-16, 24-29 and 39-255. CFF2 assigns 22-24.
    pub(in crate::cff) const fn is_reserved_in_cff(self) -> bool {
        matches!(self.0, 22..=27 | 0x0c0f..=0x0c10 | 0x0c18..=0x0c1d | 0x0c27..=0x0cff)
    }

    /// The operator whose zone pairs must precede this one's (TN 5176
    /// Table 23: OtherBlues after BlueValues, FamilyOtherBlues after
    /// FamilyBlues).
    pub(in crate::cff) const fn blue_predecessor(self) -> Option<Self> {
        match self {
            Self::OTHER_BLUES => Some(Self::BLUE_VALUES),
            Self::FAMILY_OTHER_BLUES => Some(Self::FAMILY_BLUES),
            _ => None,
        }
    }
}
