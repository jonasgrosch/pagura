//! Predictor parameters for `FlateDecode` and `LZWDecode` (7.4.4.4) and the
//! PNG filter arithmetic that the predictors and the PNG encoder share.
use crate::features;
use pagura_core::{Dictionary, Error, Result, Value};

/// Predictor parameters shared by `FlateDecode` and `LZWDecode`
/// (7.4.4.4, Table 8).
///
/// Built by [`Predictor::from_params`] or [`Predictor::new`], which enforce
/// Table 8's ranges, so decoders need not check them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Predictor {
    kind: PredictorKind,
    colors: u8,
    bits_per_component: u8,
    columns: u32,
}

impl Default for Predictor {
    fn default() -> Self {
        Self {
            kind: PredictorKind::None,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

/// The `/Predictor` value. Table 8 closes the set, so the enum is
/// exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredictorKind {
    /// 1: no prediction.
    None,
    /// 2: TIFF Predictor 2.
    Tiff,
    /// 10–15: PNG predictors, selected per row by a tag byte.
    Png,
}

const fn valid_bits(bits: u8) -> bool {
    matches!(bits, 1 | 2 | 4 | 8 | 16)
}

impl Predictor {
    /// Predictor parameters with Table 8's ranges checked.
    ///
    /// # Errors
    ///
    /// `InvalidArgument` if `colors` or `columns` is zero, or
    /// `bits_per_component` is not 1, 2, 4, 8 or 16.
    pub const fn new(
        kind: PredictorKind,
        colors: u8,
        bits_per_component: u8,
        columns: u32,
    ) -> Result<Self> {
        if colors == 0 || columns == 0 || !valid_bits(bits_per_component) {
            return Err(Error::invalid_argument("invalid predictor parameters"));
        }
        Ok(Self {
            kind,
            colors,
            bits_per_component,
            columns,
        })
    }

    /// The `/Predictor` algorithm.
    pub const fn kind(self) -> PredictorKind {
        self.kind
    }

    /// `/Colors`: interleaved color components per sample, at least 1.
    pub const fn colors(self) -> u8 {
        self.colors
    }

    /// `/BitsPerComponent`: 1, 2, 4, 8 or 16.
    pub const fn bits_per_component(self) -> u8 {
        self.bits_per_component
    }

    /// `/Columns`: samples per row, at least 1.
    pub const fn columns(self) -> u32 {
        self.columns
    }

    /// Reads predictor parameters from a `/DecodeParms` dictionary.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for values outside Table 8, or
    /// `Unsupported(predictor.dimensions)` for positive dimensions too large
    /// for the stable [`Predictor`] representation.
    pub fn from_params(params: Option<&Dictionary>) -> Result<Self> {
        let Some(params) = params else {
            return Ok(Self::default());
        };
        let integer = |key: &[u8], default: i64| match params.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Integer(value)) => Ok(*value),
            Some(_) => Err(Error::syntax("predictor parameter must be an integer")),
        };
        let kind = match integer(b"Predictor", 1)? {
            1 => PredictorKind::None,
            2 => PredictorKind::Tiff,
            10..=15 => PredictorKind::Png,
            _ => return Err(Error::syntax("invalid Predictor")),
        };
        let colors = integer(b"Colors", 1)?;
        if colors < 1 {
            return Err(Error::syntax("invalid Colors"));
        }
        let colors = u8::try_from(colors).map_err(|_| {
            Error::unsupported(
                features::PREDICTOR_DIMENSIONS,
                "Colors exceeds predictor API representation",
            )
        })?;
        let bits_per_component = u8::try_from(integer(b"BitsPerComponent", 8)?)
            .ok()
            .filter(|&bits| valid_bits(bits))
            .ok_or(Error::syntax("invalid BitsPerComponent"))?;
        let columns = integer(b"Columns", 1)?;
        if columns < 1 {
            return Err(Error::syntax("invalid Columns"));
        }
        let columns = u32::try_from(columns).map_err(|_| {
            Error::unsupported(
                features::PREDICTOR_DIMENSIONS,
                "Columns exceeds predictor API representation",
            )
        })?;
        Ok(Self {
            kind,
            colors,
            bits_per_component,
            columns,
        })
    }
}

/// A PNG filter type: the tag byte that starts each row (PNG Second Edition
/// 9.2, Table 9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum PngFilter {
    None = 0,
    Sub = 1,
    Up = 2,
    Average = 3,
    Paeth = 4,
}

impl PngFilter {
    /// Every filter type, in tag order.
    pub(crate) const ALL: [Self; 5] = [Self::None, Self::Sub, Self::Up, Self::Average, Self::Paeth];

    /// The prediction for a byte from the corresponding bytes of the pixel
    /// to its left, the row above and the pixel above-left (zero outside the
    /// image), PNG Second Edition 9.2-9.4.
    pub(crate) fn predict(self, left: u8, above: u8, upper_left: u8) -> u8 {
        match self {
            Self::None => 0,
            Self::Sub => left,
            Self::Up => above,
            // floor((left + above) / 2) without widening.
            Self::Average => (left & above) + ((left ^ above) >> 1),
            Self::Paeth => paeth(left, above, upper_left),
        }
    }
}

impl TryFrom<u8> for PngFilter {
    type Error = Error;

    fn try_from(tag: u8) -> Result<Self> {
        Self::ALL
            .get(usize::from(tag))
            .copied()
            .ok_or(Error::syntax("invalid PNG predictor tag"))
    }
}

fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    // PNG Second Edition 9.4: exact arithmetic; ties prefer left, then above.
    let p = i16::from(left) + i16::from(above) - i16::from(upper_left);
    let a = (p - i16::from(left)).abs();
    let b = (p - i16::from(above)).abs();
    let c = (p - i16::from(upper_left)).abs();
    if a <= b && a <= c {
        left
    } else if b <= c {
        above
    } else {
        upper_left
    }
}
