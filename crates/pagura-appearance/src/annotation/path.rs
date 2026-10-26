//! Bounded Polygon/PolyLine geometry (12.5.6.9, Table 181).

use super::marks::{PathOperator, Subpath};
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

pub(super) enum Geometry {
    Vertices(Vec<(f64, f64)>),
    Operators(Vec<PathOperator>),
}

impl Geometry {
    pub(super) fn subpath(&self, closed: bool) -> Subpath<'_> {
        match self {
            Self::Vertices(points) if closed => Subpath::Polygon(points),
            Self::Vertices(points) => Subpath::Polyline(points),
            Self::Operators(commands) => Subpath::Operators { commands, closed },
        }
    }
}

/// Path takes precedence over Vertices, including malformed unused Vertices.
pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<Geometry> {
    if let Some(value) = optional(document, annotation, b"Path")? {
        return operators(document, value).map(Geometry::Operators);
    }
    let vertices = optional(document, annotation, b"Vertices")?
        .and_then(Value::as_array)
        .ok_or(Error::syntax("annotation Vertices must be an array"))?;
    let (pairs, remainder) = vertices.as_chunks::<2>();
    if pairs.is_empty() || !remainder.is_empty() {
        return Err(Error::syntax(
            "annotation Vertices must contain coordinate pairs",
        ));
    }
    let mut points = reserve::<(f64, f64)>(document, pairs.len())?;
    for [x, y] in pairs {
        points.push((number(document, x)?, number(document, y)?));
    }
    Ok(Geometry::Vertices(points))
}

fn operators(document: &Document, value: &Value) -> Result<Vec<PathOperator>> {
    let arrays = value
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or(Error::syntax("annotation Path must be a nonempty array"))?;
    let mut commands = reserve::<PathOperator>(document, arrays.len())?;
    for (index, value) in arrays.iter().enumerate() {
        let operands = document
            .resolve(value)?
            .as_array()
            .ok_or(Error::syntax("annotation Path operands must be an array"))?;
        let command = match operands {
            [x, y] => {
                let point = [number(document, x)?, number(document, y)?];
                if index == 0 {
                    PathOperator::Move(point)
                } else {
                    PathOperator::Line(point)
                }
            }
            [x0, y0, x1, y1, x2, y2] if index > 0 => PathOperator::Cubic([
                number(document, x0)?,
                number(document, y0)?,
                number(document, x1)?,
                number(document, y1)?,
                number(document, x2)?,
                number(document, y2)?,
            ]),
            _ => return Err(Error::syntax("invalid annotation Path operand count")),
        };
        commands.push(command);
    }
    Ok(commands)
}

fn reserve<T>(document: &Document, count: usize) -> Result<Vec<T>> {
    let limits = document.limits();
    if count > limits.path_segments
        || count
            .checked_mul(size_of::<T>())
            .is_none_or(|bytes| bytes > limits.render_bytes)
    {
        return Err(Error::limit("annotation geometry budget exceeded"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(count)
        .map_err(|_| Error::limit("annotation geometry allocation failed"))?;
    Ok(out)
}
