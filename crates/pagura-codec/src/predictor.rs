//! Predictor parameters for `FlateDecode` and `LZWDecode` (clause 7.4.4.4).
use crate::features;
use pagura_core::{Dictionary, Error, Result, Value};

/// Predictor parameters shared by `FlateDecode` and `LZWDecode`
/// (clause 7.4.4.4, Table 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Predictor {
    /// The `/Predictor` algorithm.
    pub kind: PredictorKind,
    /// `/Colors`: interleaved colour components per sample, at least 1.
    pub colors: u8,
    /// `/BitsPerComponent`: 1, 2, 4, 8 or 16.
    pub bits_per_component: u8,
    /// `/Columns`: samples per row, at least 1.
    pub columns: u32,
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

/// The `/Predictor` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredictorKind {
    /// 1: no prediction.
    None,
    /// 2: TIFF Predictor 2.
    Tiff,
    /// 10–15: PNG predictors, selected per row by a tag byte.
    Png,
}

impl Predictor {
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
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
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
