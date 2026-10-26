//! Polygon/PolyLine appearances (12.5.6.9, Table 181).

use super::border::{Border, plain_effect};
use super::marks::{Color, Marks, Pen, color};
use super::path;
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::optional;
use pagura_core::{Dictionary, Error, Rect, Result};
use pagura_document::Document;

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    closed: bool,
) -> Result<Option<AppearanceStream>> {
    let geometry = path::read(document, annotation)?;
    if closed {
        plain_effect(document, annotation)?;
    } else {
        no_endings(document, annotation)?;
    }
    let border = Border::read(document, annotation)?;
    let mut marks = Marks::new(document, annotation);
    let path = [geometry.subpath(closed)];
    if closed && let Some(interior) = color(document, annotation, b"IC", None)? {
        marks.fill(&path, &interior)?;
    }
    if border.width != 0.0
        && let Some(color) = color(document, annotation, b"C", Some(Color::black()))?
    {
        marks.stroke(
            &path,
            &color,
            Pen {
                width: border.width,
                dash: &border.dash,
            },
        )?;
    }
    marks.finish(rect)
}

pub(super) fn no_endings(document: &Document, annotation: &Dictionary) -> Result<()> {
    let Some(value) = optional(document, annotation, b"LE")? else {
        return Ok(());
    };
    let values = value
        .as_array()
        .ok_or(Error::syntax("line endings must be an array"))?;
    let [start, end] = values else {
        return Err(Error::syntax("line endings require two names"));
    };
    for value in [start, end] {
        let name = document
            .resolve(value)?
            .as_name()
            .ok_or(Error::syntax("line ending must be a name"))?;
        if name != b"None" {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "line ending generation is not implemented",
            ));
        }
    }
    Ok(())
}
