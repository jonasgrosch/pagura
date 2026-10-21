//! Shading dictionaries and streams (8.7.4.5) read into
//! [`Shading`](crate::model::Shading).

mod mesh;

pub(crate) use mesh::retained_bytes;

use super::colorspace::Colors;
use super::function::{self, ReadFunction};
use super::numbers::{GEOMETRY, array, entry};
use crate::features;
use crate::model::{Shading, ShadingFunction, ShadingKind};
use pagura_color::{ColorSpace, Components, MAX_COMPONENTS};
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_document::Document;
use pagura_raster::{Point, Transform};
use std::sync::Arc;

/// Reads the shading `value` with colour spaces named in `resources`.
///
/// # Errors
///
/// `Syntax` for malformed shadings, `Unsupported` for singular function
/// matrices, `Limit` for budgets, and the errors of colour spaces and
/// functions.
pub(crate) fn read(
    colors: &mut Colors<'_>,
    resources: &Dictionary,
    value: &Value,
) -> Result<Shading> {
    let document = colors.document();
    let d = document.resolve_dictionary(value)?;
    let kind = entry(document, d, b"ShadingType")?
        .and_then(Value::as_integer)
        .ok_or(Error::syntax("shading requires ShadingType"))?;
    if !(1..=7).contains(&kind) {
        return Err(Error::syntax("invalid ShadingType"));
    }
    let space = colors.resolve(
        resources,
        d.get(b"ColorSpace")
            .ok_or(Error::syntax("shading requires ColorSpace"))?,
    )?;
    let indexed = matches!(space.as_ref(), ColorSpace::Indexed(_));
    if matches!(space.as_ref(), ColorSpace::Pattern(_))
        || (kind < 4 && indexed)
        || (indexed && d.contains_key(b"Function"))
    {
        return Err(Error::syntax("analytic shading cannot use Indexed"));
    }
    let components = space.components();
    if components > MAX_COMPONENTS {
        return Err(Error::limit(
            "shading component count exceeds implementation limit (Annex C)",
        ));
    }
    let bbox = d
        .get(b"BBox")
        .map(|v| {
            let n = array::<4>(document, v, GEOMETRY)?;
            Rect::from_corners(n[0], n[1], n[2], n[3]).ok_or(Error::syntax("invalid shading BBox"))
        })
        .transpose()?;
    let antialias = d
        .get(b"AntiAlias")
        .map(|v| {
            document
                .resolve(v)?
                .as_bool()
                .ok_or(Error::syntax("shading AntiAlias must be boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    let background = d
        .get(b"Background")
        .map(|v| background(document, v, components))
        .transpose()?;
    let (kind, function) = if kind >= 4 {
        mesh::read(document, d, value, kind, components)?
    } else {
        let (geometry, domain) = geometry(document, d, kind)?;
        let function = functions(
            document,
            d.get(b"Function")
                .ok_or(Error::syntax("shading requires Function"))?,
            &domain,
            components,
        )?;
        (geometry, Some(function))
    };
    Ok(Shading {
        object: value.as_reference(),
        space,
        background,
        bbox,
        antialias,
        kind,
        function,
    })
}

fn background(document: &Document, value: &Value, components: usize) -> Result<Components> {
    let values = document
        .resolve(value)?
        .as_array()
        .filter(|v| v.len() == components)
        .ok_or(Error::syntax("wrong shading Background dimension"))?;
    let mut out = [0.0; MAX_COMPONENTS];
    for (out, v) in out.iter_mut().zip(values) {
        *out = document
            .resolve(v)?
            .as_number()
            .ok_or(Error::syntax("shading Background must be numeric"))?;
    }
    Ok(Components::from_slice(
        out.get(..components).unwrap_or_default(),
    ))
}

/// The geometry of an analytic shading and its function domain.
fn geometry(
    document: &Document,
    d: &Dictionary,
    kind: i64,
) -> Result<(ShadingKind, Vec<[f64; 2]>)> {
    if kind == 1 {
        let domain = optional::<4>(document, d, b"Domain", [0.0, 1.0, 0.0, 1.0])?;
        if domain[0] >= domain[1] || domain[2] >= domain[3] {
            return Err(Error::syntax("invalid shading Domain"));
        }
        let m = optional::<6>(document, d, b"Matrix", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])?;
        let matrix = Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]);
        if matrix.inverse().is_none() {
            return Err(Error::unsupported(
                features::SHADING_TRANSFORM,
                "singular shading Matrix",
            ));
        }
        return Ok((
            ShadingKind::Function { domain, matrix },
            vec![[domain[0], domain[1]], [domain[2], domain[3]]],
        ));
    }
    let domain = optional::<2>(document, d, b"Domain", [0.0, 1.0])?;
    if domain[0] >= domain[1] {
        return Err(Error::syntax("invalid shading Domain"));
    }
    let extend = d
        .get(b"Extend")
        .map(|v| {
            let values = document
                .resolve(v)?
                .as_array()
                .filter(|v| v.len() == 2)
                .ok_or(Error::syntax("shading Extend requires two flags"))?;
            let mut result = [false; 2];
            for (out, v) in result.iter_mut().zip(values) {
                *out = document
                    .resolve(v)?
                    .as_bool()
                    .ok_or(Error::syntax("shading Extend must be boolean"))?;
            }
            Ok(result)
        })
        .transpose()?
        .unwrap_or([false; 2]);
    let coords = d
        .get(b"Coords")
        .ok_or(Error::syntax("shading requires Coords"))?;
    let geometry = if kind == 2 {
        let c = array::<4>(document, coords, GEOMETRY)?;
        ShadingKind::Axial {
            start: Point::new(c[0], c[1]),
            end: Point::new(c[2], c[3]),
            domain,
            extend,
        }
    } else {
        let c = array::<6>(document, coords, GEOMETRY)?;
        if c[2] < 0.0 || c[5] < 0.0 {
            return Err(Error::syntax("negative shading radius"));
        }
        ShadingKind::Radial {
            start: Point::new(c[0], c[1]),
            start_radius: c[2],
            end: Point::new(c[3], c[4]),
            end_radius: c[5],
            domain,
            extend,
        }
    };
    Ok((geometry, vec![domain]))
}

/// A shading's `Function`: one function with `outputs` outputs or an array
/// of single-output functions, each covering `domain`.
fn functions(
    document: &Document,
    value: &Value,
    domain: &[[f64; 2]],
    outputs: usize,
) -> Result<ShadingFunction> {
    let read = |value: &Value, outputs: usize| -> Result<ReadFunction> {
        let function = function::read(document, value, domain.len(), outputs)?;
        if !function.covers(domain) {
            return Err(Error::syntax("function domain does not cover shading"));
        }
        Ok(function)
    };
    if let Some(values) = document.resolve(value)?.as_array() {
        if values.len() != outputs {
            return Err(Error::syntax("wrong shading Function array dimension"));
        }
        let functions = values
            .iter()
            .map(|v| read(v, 1).map(|f| f.function))
            .collect::<Result<Vec<_>>>()?;
        Ok(ShadingFunction::PerComponent(Arc::from(functions)))
    } else {
        Ok(ShadingFunction::Single(read(value, outputs)?.function))
    }
}

fn optional<const N: usize>(
    document: &Document,
    d: &Dictionary,
    key: &[u8],
    default: [f64; N],
) -> Result<[f64; N]> {
    d.get(key)
        .map(|v| array::<N>(document, v, GEOMETRY))
        .transpose()
        .map(|v| v.unwrap_or(default))
}
