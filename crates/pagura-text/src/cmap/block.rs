//! CMap mapping blocks (9.7.5.4, 9.10.3; TN5014 5.3–5.4).
use crate::cid::map::Step;

/// A `begin…`/`end…` block of a CMap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Block {
    Codespace,
    CidChar,
    CidRange,
    NotdefChar,
    NotdefRange,
    BfChar,
    BfRange,
}

/// What a block's entries define.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Codespace,
    /// Codes to CIDs, in an Encoding CMap.
    Cid(Step),
    /// Codes to Unicode, in a ToUnicode CMap.
    Unicode,
}

impl Block {
    pub(super) fn parse(operator: &[u8]) -> Option<Self> {
        Some(match operator {
            b"begincodespacerange" => Self::Codespace,
            b"begincidchar" => Self::CidChar,
            b"begincidrange" => Self::CidRange,
            b"beginnotdefchar" => Self::NotdefChar,
            b"beginnotdefrange" => Self::NotdefRange,
            b"beginbfchar" => Self::BfChar,
            b"beginbfrange" => Self::BfRange,
            _ => return None,
        })
    }

    pub(super) const fn end(self) -> &'static [u8] {
        match self {
            Self::Codespace => b"endcodespacerange",
            Self::CidChar => b"endcidchar",
            Self::CidRange => b"endcidrange",
            Self::NotdefChar => b"endnotdefchar",
            Self::NotdefRange => b"endnotdefrange",
            Self::BfChar => b"endbfchar",
            Self::BfRange => b"endbfrange",
        }
    }

    /// Whether entries give a first and a last code rather than one code.
    pub(super) const fn is_range(self) -> bool {
        matches!(
            self,
            Self::Codespace | Self::CidRange | Self::NotdefRange | Self::BfRange
        )
    }

    pub(super) const fn target(self) -> Target {
        match self {
            Self::Codespace => Target::Codespace,
            Self::CidChar | Self::CidRange => Target::Cid(Step::Increment),
            Self::NotdefChar | Self::NotdefRange => Target::Cid(Step::Constant),
            Self::BfChar | Self::BfRange => Target::Unicode,
        }
    }
}
