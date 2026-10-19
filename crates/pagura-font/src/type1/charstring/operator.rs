//! Type 1 charstring commands (Type 1 6.4 and 6.5).

/// A one-byte command. Bytes 32-255 encode numbers instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Operator {
    Hstem = 1,
    Vstem = 3,
    Vmoveto = 4,
    Rlineto = 5,
    Hlineto = 6,
    Vlineto = 7,
    Rrcurveto = 8,
    Closepath = 9,
    Callsubr = 10,
    Return = 11,
    Escape = 12,
    Hsbw = 13,
    Endchar = 14,
    Rmoveto = 21,
    Hmoveto = 22,
    Vhcurveto = 30,
    Hvcurveto = 31,
}

impl TryFrom<u8> for Operator {
    type Error = u8;

    fn try_from(byte: u8) -> Result<Self, u8> {
        Ok(match byte {
            1 => Self::Hstem,
            3 => Self::Vstem,
            4 => Self::Vmoveto,
            5 => Self::Rlineto,
            6 => Self::Hlineto,
            7 => Self::Vlineto,
            8 => Self::Rrcurveto,
            9 => Self::Closepath,
            10 => Self::Callsubr,
            11 => Self::Return,
            12 => Self::Escape,
            13 => Self::Hsbw,
            14 => Self::Endchar,
            21 => Self::Rmoveto,
            22 => Self::Hmoveto,
            30 => Self::Vhcurveto,
            31 => Self::Hvcurveto,
            _ => return Err(byte),
        })
    }
}

/// An escaped (12 x) command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Escape {
    Dotsection = 0,
    Vstem3 = 1,
    Hstem3 = 2,
    Seac = 6,
    Sbw = 7,
    Div = 12,
    Callothersubr = 16,
    Pop = 17,
    Setcurrentpoint = 33,
}

impl TryFrom<u8> for Escape {
    type Error = u8;

    fn try_from(byte: u8) -> Result<Self, u8> {
        Ok(match byte {
            0 => Self::Dotsection,
            1 => Self::Vstem3,
            2 => Self::Hstem3,
            6 => Self::Seac,
            7 => Self::Sbw,
            12 => Self::Div,
            16 => Self::Callothersubr,
            17 => Self::Pop,
            33 => Self::Setcurrentpoint,
            _ => return Err(byte),
        })
    }
}
