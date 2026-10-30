//! InkList and PDF 2.0 Path (12.5.6.13, Table 185). Reader policy joins
//! InkList samples with straight segments, butt caps and miter joins.

use super::border::Border;
use super::marks::{Color, Marks, PathOperator, Pen, Subpath, color};
use super::path;
use crate::stream::AppearanceStream;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Rect, Result};
use pagura_document::Document;

enum Geometry {
    Samples(Vec<Vec<(f64, f64)>>),
    Operators(Vec<PathOperator>),
}

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    // Table 185's errata requires ignoring InkList when Path is present.
    // Appearance generation is not required-key conformance validation:
    // do not resolve or allocate unused legacy geometry.
    let geometry = if let Some(value) = optional(document, annotation, b"Path")? {
        Geometry::Operators(path::operators(document, value)?)
    } else {
        Geometry::Samples(paths(document, annotation)?)
    };
    let border = Border::read(document, annotation)?;
    let color = color(document, annotation, b"C", Some(Color::black()))?;
    let mut marks = Marks::new(document, annotation);
    if border.width != 0.0
        && let Some(color) = color
    {
        let pen = Pen {
            width: border.width,
            dash: &border.dash,
        };
        match &geometry {
            Geometry::Samples(paths) => {
                let mut strokes = Vec::new();
                strokes
                    .try_reserve_exact(paths.len())
                    .map_err(|_| Error::limit("ink path allocation failed"))?;
                strokes.extend(paths.iter().map(|path| Subpath::Polyline(path)));
                marks.stroke(&strokes, &color, pen)?;
            }
            Geometry::Operators(commands) => {
                marks.stroke(
                    &[Subpath::Operators {
                        commands,
                        closed: false,
                    }],
                    &color,
                    pen,
                )?;
            }
        }
    }
    marks.finish(rect)
}

/// Bound aggregate path storage and work before copying coordinate pairs.
/// Indirect arrays may repeat; their combined size, not just each parsed
/// object's size, must fit. The budget includes the later borrowed subpaths.
fn paths(document: &Document, annotation: &Dictionary) -> Result<Vec<Vec<(f64, f64)>>> {
    let values = optional(document, annotation, b"InkList")?
        .and_then(pagura_core::Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or(Error::syntax("InkList must be a nonempty array"))?;
    let limits = document.limits();
    let mut bytes = values
        .len()
        .checked_mul(size_of::<Vec<(f64, f64)>>() + size_of::<Subpath<'_>>())
        .filter(|bytes| *bytes <= limits.render_bytes)
        .ok_or(Error::limit("ink path storage budget exceeded"))?;
    let mut segments = values.len();
    if segments > limits.path_segments {
        return Err(Error::limit("ink path segment budget exceeded"));
    }
    let mut paths = Vec::new();
    paths
        .try_reserve_exact(values.len())
        .map_err(|_| Error::limit("ink path allocation failed"))?;
    for value in values {
        let coordinates = document
            .resolve(value)?
            .as_array()
            .ok_or(Error::syntax("ink path must be an array"))?;
        let (pairs, remainder) = coordinates.as_chunks::<2>();
        if pairs.is_empty() || !remainder.is_empty() {
            return Err(Error::syntax("ink path must contain coordinate pairs"));
        }
        segments = segments
            .checked_add(pairs.len().saturating_sub(1))
            .filter(|count| *count <= limits.path_segments)
            .ok_or(Error::limit("ink path segment budget exceeded"))?;
        bytes = pairs
            .len()
            .checked_mul(size_of::<(f64, f64)>())
            .and_then(|more| bytes.checked_add(more))
            .filter(|total| *total <= limits.render_bytes)
            .ok_or(Error::limit("ink path storage budget exceeded"))?;
        let mut path = Vec::new();
        path.try_reserve_exact(pairs.len())
            .map_err(|_| Error::limit("ink point allocation failed"))?;
        for [x, y] in pairs {
            path.push((number(document, x)?, number(document, y)?));
        }
        paths.push(path);
    }
    Ok(paths)
}
