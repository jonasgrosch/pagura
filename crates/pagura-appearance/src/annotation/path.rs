//! Bounded Polygon/PolyLine geometry and annotation Path (Tables 181/185).

use super::ending::Endpoint;
use super::marks::{PathOperator, Subpath};
use super::polygon::Kind;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

pub(super) enum Geometry {
    Vertices(Vec<(f64, f64)>),
    Operators(Vec<PathOperator>),
}

impl Geometry {
    /// Outward endpoint directions use the first/last nonzero source
    /// derivative, not the chords used later by a renderer.
    pub(super) fn endpoints(&self) -> Result<[Endpoint; 2]> {
        let mut ends = [None; 2];
        let mut first = None;
        let mut point = (0.0, 0.0);
        let mut segment = |from, controls: [(f64, f64); 3]| -> Result<()> {
            if ends[0].is_none()
                && let Some(adjacent) = controls.iter().find(|&&p| p != from)
            {
                ends[0] = Some(Endpoint::new(from, *adjacent)?);
            }
            let [c1, c2, end] = controls;
            if let Some(adjacent) = [c2, c1, from].into_iter().find(|&p| p != end) {
                ends[1] = Some(Endpoint::new(end, adjacent)?);
            }
            Ok(())
        };
        match self {
            Self::Vertices(points) => {
                first = points.first().copied();
                for pair in points.windows(2) {
                    segment(pair[0], [pair[1]; 3])?;
                }
            }
            Self::Operators(commands) => {
                for command in commands {
                    match *command {
                        PathOperator::Move([x, y]) => {
                            point = (x, y);
                            first = Some(point);
                        }
                        PathOperator::Line([x, y]) => {
                            segment(point, [(x, y); 3])?;
                            point = (x, y);
                        }
                        PathOperator::Cubic([x1, y1, x2, y2, x3, y3]) => {
                            segment(point, [(x1, y1), (x2, y2), (x3, y3)])?;
                            point = (x3, y3);
                        }
                    }
                }
            }
        }
        let first = first.ok_or(Error::syntax("annotation path has no endpoint"))?;
        let fallback = Endpoint::new(first, first)?;
        Ok([ends[0].unwrap_or(fallback), ends[1].unwrap_or(fallback)])
    }

    pub(super) fn subpath(&self, kind: Kind) -> Subpath<'_> {
        match self {
            Self::Vertices(points) if kind == Kind::Polygon => Subpath::Polygon(points),
            Self::Vertices(points) => Subpath::Polyline(points),
            Self::Operators(commands) => Subpath::Operators {
                commands,
                closed: kind == Kind::Polygon,
            },
        }
    }
}

/// Table 181's errata requires ignoring Vertices when Path is present,
/// including malformed or unresolved unused entries. Required-key document
/// conformance validation is separate from appearance generation.
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

/// A resolved PDF 2.0 Path array: one moveto, then lines or cubics.
pub(super) fn operators(document: &Document, value: &Value) -> Result<Vec<PathOperator>> {
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
