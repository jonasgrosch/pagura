//! Caret annotations (12.5.6.11, Table 183).
//!
//! Reader artwork uses a filled chevron and an optional geometric pilcrow in
//! its upper-right space. Both scale to the RD-inset bounds in a 16-unit grid
//! and share one fill. They need no fonts and retain the annotation BBox.

use super::border::inset_bounds;
use super::marks::{Color, Marks, Subpath, color};
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::optional;
use pagura_core::{Dictionary, Error, Rect, Result};
use pagura_document::Document;

const CHEVRON: &[(f64, f64)] = &[(0., 0.), (8., 16.), (16., 0.), (8., 12.)];
const PARAGRAPH: &[(f64, f64)] = &[
    (11., 15.),
    (12., 16.),
    (16., 16.),
    (16., 8.),
    (15., 8.),
    (15., 15.),
    (14., 15.),
    (14., 8.),
    (13., 8.),
    (13., 12.),
    (12., 12.),
    (11., 13.),
];

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let bounds = inset_bounds(document, annotation, rect)?;
    let paragraph = match optional(document, annotation, b"Sy")? {
        None => false,
        Some(value) => match value
            .as_name()
            .ok_or(Error::syntax("caret Sy must be a name"))?
            .as_bytes()
        {
            b"None" => false,
            b"P" => true,
            _ => {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "unknown caret symbol",
                ));
            }
        },
    };
    let segments = CHEVRON.len() + if paragraph { PARAGRAPH.len() } else { 0 };
    let limits = document.limits();
    if segments > limits.path_segments || segments > limits.operations {
        return Err(Error::limit("caret geometry budget exceeded"));
    }
    let mut marks = Marks::new(document, annotation);
    if let Some(color) = color(document, annotation, b"C", Some(Color::black()))? {
        marks.transform([
            bounds.width() / 16.0,
            0.0,
            0.0,
            bounds.height() / 16.0,
            bounds.x0(),
            bounds.y0(),
        ])?;
        let paths = [Subpath::Polygon(CHEVRON), Subpath::Polygon(PARAGRAPH)];
        marks.fill(&paths[..if paragraph { 2 } else { 1 }], &color)?;
    }
    marks.finish(rect)
}
