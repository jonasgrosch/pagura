//! Pattern dictionaries (8.7.2–8.7.3, 8.7.4.2): kind, matrix, and the
//! tiling cell's parameters.

use super::numbers::{GEOMETRY, array, entry};
use crate::features;
use crate::model::TilingColors;
use pagura_core::{Dictionary, Error, ObjectId, Rect, Result, Value};
use pagura_document::Document;
use pagura_raster::Transform;

/// A tiling pattern's parameters (Table 75).
#[derive(Debug, Clone)]
pub(crate) struct Tiling<'a> {
    pub(crate) id: ObjectId,
    pub(crate) bbox: Rect,
    pub(crate) step: [f64; 2],
    pub(crate) colors: TilingColors,
    pub(crate) resources: &'a Dictionary,
}

/// What a pattern resource is.
#[derive(Debug, Clone)]
pub(crate) enum Kind<'a> {
    /// `PatternType 1`.
    Tiling(Tiling<'a>),
    /// `PatternType 2`, with its `Shading` value.
    Shading(&'a Value),
}

/// A pattern resource: its kind and pattern space to default user space.
#[derive(Debug, Clone)]
pub(crate) struct Pattern<'a> {
    pub(crate) kind: Kind<'a>,
    pub(crate) matrix: Transform,
}

/// Reads the pattern `name` of `resources`, anchored at `initial`, the CTM
/// at the start of the content stream that uses it (8.7.2).
///
/// # Errors
///
/// `Syntax` for malformed patterns, `Unsupported` for shading patterns with
/// graphics state parameters, and `Limit` for an overflowing matrix.
pub(crate) fn read<'a>(
    document: &'a Document,
    resources: &'a Dictionary,
    name: &[u8],
    initial: Transform,
) -> Result<Pattern<'a>> {
    let value = resources
        .get(b"Pattern")
        .map(|v| document.resolve_dictionary(v))
        .transpose()?
        .and_then(|d| d.get(name))
        .ok_or(Error::syntax("undefined pattern resource"))?;
    let d = document.resolve_dictionary(value)?;
    let kind = entry(document, d, b"PatternType")?
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
            features::PATTERN_STATE,
            "pattern rendering parameters are not implemented",
        ));
    }
    let matrix = d
        .get(b"Matrix")
        .map(|v| {
            let m = array::<6>(document, v, GEOMETRY)?;
            Ok(Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
        })
        .transpose()?
        .unwrap_or(Transform::IDENTITY)
        .then(initial);
    if !matrix.is_finite() {
        return Err(Error::limit("pattern transform overflow"));
    }
    let kind = if kind == 1 {
        Kind::Tiling(tiling(document, value, d)?)
    } else {
        Kind::Shading(
            d.get(b"Shading")
                .ok_or(Error::syntax("pattern requires Shading"))?,
        )
    };
    Ok(Pattern { kind, matrix })
}

fn tiling<'a>(document: &'a Document, value: &Value, d: &'a Dictionary) -> Result<Tiling<'a>> {
    let id = value
        .as_reference()
        .ok_or(Error::syntax("tiling pattern must be a stream"))?;
    if !document.object(id)?.is_stream() {
        return Err(Error::syntax("tiling pattern must be a stream"));
    }
    let integer = |key: &[u8]| {
        entry(document, d, key)?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("missing pattern type parameter"))
    };
    let colors = match integer(b"PaintType")? {
        1 => TilingColors::Colored,
        2 => TilingColors::Uncolored,
        _ => return Err(Error::syntax("invalid PaintType")),
    };
    if !matches!(integer(b"TilingType")?, 1..=3) {
        return Err(Error::syntax("invalid TilingType"));
    }
    let b = array::<4>(
        document,
        d.get(b"BBox")
            .ok_or(Error::syntax("pattern requires BBox"))?,
        GEOMETRY,
    )?;
    let bbox =
        Rect::from_corners(b[0], b[1], b[2], b[3]).ok_or(Error::syntax("invalid pattern BBox"))?;
    let mut step = [0.0; 2];
    for (out, key) in step.iter_mut().zip([b"XStep", b"YStep"]) {
        *out = entry(document, d, key)?
            .and_then(Value::as_number)
            .filter(|v| *v != 0.0)
            .ok_or(Error::syntax("pattern step must be nonzero"))?
            .abs();
    }
    let resources = document.resolve_dictionary(
        d.get(b"Resources")
            .ok_or(Error::syntax("pattern requires Resources"))?,
    )?;
    Ok(Tiling {
        id,
        bbox,
        step,
        colors,
        resources,
    })
}
