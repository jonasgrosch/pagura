//! Tiling and shading patterns, anchored to the initial user space of the
//! stream that selects them (8.7.2).

use crate::icc::ColorContext;
use crate::numbers::{self, GEOMETRY};
use crate::shading::Shading;
use pagura_core::{Dictionary, Error, ObjectId, Rect, Result, Value};
use pagura_raster::Transform;
use std::sync::Arc;

/// A selected pattern, ready to paint (8.7.2). Tiling cells are interpreted
/// before a `Pattern` exists, so a pattern is never half-built.
#[derive(Debug)]
pub(crate) struct Pattern {
    pub(crate) kind: Kind,
    pub(crate) transform: Transform,
}
#[derive(Debug)]
pub(crate) enum Kind {
    Shading(Arc<Shading>),
    Tiling(Tiling),
}
/// An interpreted tiling cell and how it repeats (8.7.3, Table 74).
#[derive(Debug)]
pub(crate) struct Tiling {
    pub(crate) bbox: Rect,
    pub(crate) step: [f64; 2],
    pub(crate) spacing: TilingType,
    pub(crate) commands: Vec<crate::display::Command>,
}

/// A pattern resource as parsed, before any tiling cell is interpreted.
#[derive(Debug)]
pub(crate) struct Definition {
    /// The pattern object, when it is indirect; it identifies repeated
    /// selections of one pattern.
    pub(crate) object: Option<ObjectId>,
    pub(crate) transform: Transform,
    pub(crate) kind: DefinitionKind,
}
#[derive(Debug)]
pub(crate) enum DefinitionKind {
    Shading(Arc<Shading>),
    Tiling(TilingDefinition),
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct TilingDefinition {
    pub(crate) id: ObjectId,
    pub(crate) bbox: Rect,
    pub(crate) step: [f64; 2],
    pub(crate) paint: PaintType,
    pub(crate) spacing: TilingType,
}
impl TilingDefinition {
    pub(crate) fn tiling(self, commands: Vec<crate::display::Command>) -> Tiling {
        Tiling {
            bbox: self.bbox,
            step: self.step,
            spacing: self.spacing,
            commands,
        }
    }
}

/// Whether a tiling cell specifies its own colours (Table 74 `PaintType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaintType {
    /// 1: the cell's content specifies colours.
    Colored,
    /// 2: the cell is a stencil painted in the colour given with `scn`.
    Uncolored,
}
impl TryFrom<i64> for PaintType {
    type Error = Error;
    fn try_from(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::Colored),
            2 => Ok(Self::Uncolored),
            _ => Err(Error::syntax("invalid PaintType")),
        }
    }
}

/// How tiling-cell spacing adjusts to the device (Table 74 `TilingType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TilingType {
    /// 1: constant spacing, distorting the cell by at most a device pixel.
    ConstantSpacing,
    /// 2: no distortion; spacing may vary by a device pixel.
    NoDistortion,
    /// 3: constant spacing with faster tiling, allowing more distortion.
    Faster,
}
impl TryFrom<i64> for TilingType {
    type Error = Error;
    fn try_from(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::ConstantSpacing),
            2 => Ok(Self::NoDistortion),
            3 => Ok(Self::Faster),
            _ => Err(Error::syntax("invalid TilingType")),
        }
    }
}
impl Definition {
    pub(crate) fn parse(
        document: &ColorContext<'_>,
        resources: &Dictionary,
        name: &[u8],
        initial: Transform,
    ) -> Result<Self> {
        let value = resources
            .get(b"Pattern")
            .map(|v| document.resolve_dictionary(v))
            .transpose()?
            .and_then(|d| d.get(name))
            .ok_or(Error::syntax("undefined pattern resource"))?;
        let d = document.resolve_dictionary(value)?;
        let kind = d
            .get(b"PatternType")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("pattern requires PatternType"))?;
        if !matches!(kind, 1 | 2) {
            return Err(Error::syntax("invalid PatternType"));
        }
        if kind == 2
            && let Some(state) = d.get(b"ExtGState")
            && !document.resolve_dictionary(state)?.is_empty()
        {
            return Err(Error::unsupported(
                crate::features::PATTERN_STATE,
                "pattern rendering parameters are not implemented",
            ));
        }
        let transform = d
            .get(b"Matrix")
            .map(|v| {
                let m = numbers::array::<6>(document.document(), v, GEOMETRY)?;
                Ok(Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
            })
            .transpose()?
            .unwrap_or(Transform::IDENTITY)
            .then(initial);
        if !transform.is_finite() {
            return Err(Error::limit("pattern transform overflow"));
        }
        let kind = if kind == 1 {
            DefinitionKind::Tiling(TilingDefinition::parse(document, value, d)?)
        } else {
            DefinitionKind::Shading(Arc::new(Shading::parse(
                document,
                resources,
                d.get(b"Shading")
                    .ok_or(Error::syntax("pattern requires Shading"))?,
            )?))
        };
        Ok(Self {
            object: value.as_reference(),
            transform,
            kind,
        })
    }
}
impl TilingDefinition {
    fn parse(document: &ColorContext<'_>, value: &Value, d: &Dictionary) -> Result<Self> {
        let id = value
            .as_reference()
            .ok_or(Error::syntax("tiling pattern must be a stream"))?;
        if !document.object(id)?.is_stream() {
            return Err(Error::syntax("tiling pattern must be a stream"));
        }
        let integer = |key: &[u8]| {
            d.get(key)
                .map(|v| document.resolve(v))
                .transpose()?
                .and_then(Value::as_integer)
                .ok_or(Error::syntax("missing pattern type parameter"))
        };
        let paint = PaintType::try_from(integer(b"PaintType")?)?;
        let spacing = TilingType::try_from(integer(b"TilingType")?)?;
        let b = numbers::array::<4>(
            document.document(),
            d.get(b"BBox")
                .ok_or(Error::syntax("pattern requires BBox"))?,
            GEOMETRY,
        )?;
        let bbox = Rect::from_corners(b[0], b[1], b[2], b[3])
            .ok_or(Error::syntax("invalid pattern BBox"))?;
        let mut step = [0.0; 2];
        for (out, key) in step.iter_mut().zip([b"XStep", b"YStep"]) {
            *out = d
                .get(key)
                .map(|v| document.resolve(v))
                .transpose()?
                .and_then(Value::as_number)
                .filter(|v| *v != 0.0)
                .ok_or(Error::syntax("pattern step must be nonzero"))?
                .abs();
        }
        document.resolve_dictionary(
            d.get(b"Resources")
                .ok_or(Error::syntax("pattern requires Resources"))?,
        )?;
        Ok(Self {
            id,
            bbox,
            step,
            paint,
            spacing,
        })
    }
}
