//! Shading patterns anchored to their parent stream's initial user space (8.7.2).

use crate::icc::ColorContext;
use crate::shading::{Shading, numbers};
use pagura_core::{Dictionary, Error, ObjectId, Rect, Result, Value};
use pagura_raster::Transform;
use std::sync::Arc;

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
#[derive(Debug)]
pub(crate) struct Tiling {
    pub(crate) id: ObjectId,
    pub(crate) bbox: Rect,
    pub(crate) step: [f64; 2],
    pub(crate) uncolored: bool,
    pub(crate) spacing: usize,
    pub(crate) color: Option<crate::color::Color>,
    pub(crate) commands: Vec<crate::display::Command>,
}
impl Pattern {
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
                let m = numbers::<6>(document, v)?;
                Ok(Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
            })
            .transpose()?
            .unwrap_or(Transform::IDENTITY)
            .then(initial);
        if !transform.is_finite() {
            return Err(Error::limit("pattern transform overflow"));
        }
        let kind = if kind == 1 {
            Kind::Tiling(Tiling::parse(document, value, d)?)
        } else {
            Kind::Shading(Arc::new(Shading::parse(
                document,
                resources,
                d.get(b"Shading")
                    .ok_or(Error::syntax("pattern requires Shading"))?,
            )?))
        };
        Ok(Self { kind, transform })
    }
}
impl Tiling {
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
        let uncolored = match integer(b"PaintType")? {
            1 => false,
            2 => true,
            _ => return Err(Error::syntax("invalid PaintType")),
        };
        let spacing = usize::try_from(integer(b"TilingType")?)
            .map_err(|_| Error::syntax("invalid TilingType"))?;
        if !matches!(spacing, 1..=3) {
            return Err(Error::syntax("invalid TilingType"));
        }
        let b = numbers::<4>(
            document,
            d.get(b"BBox")
                .ok_or(Error::syntax("pattern requires BBox"))?,
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
            uncolored,
            spacing,
            color: None,
            commands: Vec::new(),
        })
    }
}
