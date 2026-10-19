//! Type 2 charstring operators (TN 5177 Appendix A; CFF2 adds `vsindex`
//! and `blend` and drops `return` and `endchar`).

/// A one-byte operator. Bytes 28 and 32-255 encode numbers instead.
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
    Callsubr = 10,
    Return = 11,
    Escape = 12,
    Endchar = 14,
    Vsindex = 15,
    Blend = 16,
    Hstemhm = 18,
    Hintmask = 19,
    Cntrmask = 20,
    Rmoveto = 21,
    Hmoveto = 22,
    Vstemhm = 23,
    Rcurveline = 24,
    Rlinecurve = 25,
    Vvcurveto = 26,
    Hhcurveto = 27,
    Callgsubr = 29,
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
            10 => Self::Callsubr,
            11 => Self::Return,
            12 => Self::Escape,
            14 => Self::Endchar,
            15 => Self::Vsindex,
            16 => Self::Blend,
            18 => Self::Hstemhm,
            19 => Self::Hintmask,
            20 => Self::Cntrmask,
            21 => Self::Rmoveto,
            22 => Self::Hmoveto,
            23 => Self::Vstemhm,
            24 => Self::Rcurveline,
            25 => Self::Rlinecurve,
            26 => Self::Vvcurveto,
            27 => Self::Hhcurveto,
            29 => Self::Callgsubr,
            30 => Self::Vhcurveto,
            31 => Self::Hvcurveto,
            _ => return Err(byte),
        })
    }
}

/// An escaped (12 x) operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Escape {
    Dotsection = 0,
    And = 3,
    Or = 4,
    Not = 5,
    Abs = 9,
    Add = 10,
    Sub = 11,
    Div = 12,
    Neg = 14,
    Eq = 15,
    Drop = 18,
    Put = 20,
    Get = 21,
    Ifelse = 22,
    Random = 23,
    Mul = 24,
    Sqrt = 26,
    Dup = 27,
    Exch = 28,
    Index = 29,
    Roll = 30,
    Hflex = 34,
    Flex = 35,
    Hflex1 = 36,
    Flex1 = 37,
}

impl TryFrom<u8> for Escape {
    type Error = u8;

    fn try_from(byte: u8) -> Result<Self, u8> {
        Ok(match byte {
            0 => Self::Dotsection,
            3 => Self::And,
            4 => Self::Or,
            5 => Self::Not,
            9 => Self::Abs,
            10 => Self::Add,
            11 => Self::Sub,
            12 => Self::Div,
            14 => Self::Neg,
            15 => Self::Eq,
            18 => Self::Drop,
            20 => Self::Put,
            21 => Self::Get,
            22 => Self::Ifelse,
            23 => Self::Random,
            24 => Self::Mul,
            26 => Self::Sqrt,
            27 => Self::Dup,
            28 => Self::Exch,
            29 => Self::Index,
            30 => Self::Roll,
            34 => Self::Hflex,
            35 => Self::Flex,
            36 => Self::Hflex1,
            37 => Self::Flex1,
            _ => return Err(byte),
        })
    }
}
