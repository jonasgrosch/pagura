//! Polygon/PolyLine appearances (12.5.6.9, Table 181).

use super::border::Border;
use super::cloud;
use super::ending;
use super::marks::{Color, Marks, Pen, color};
use super::path;
use crate::stream::AppearanceStream;
use pagura_core::{Dictionary, Rect, Result};
use pagura_document::Document;

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    closed: bool,
) -> Result<Option<AppearanceStream>> {
    let geometry = path::read(document, annotation)?;
    let effect = if closed {
        cloud::Effect::read(document, annotation)?
    } else {
        None
    };
    let endings = if closed {
        None
    } else {
        Some(ending::read(document, annotation)?)
    };
    let border = Border::read(document, annotation)?;
    let cloud = effect
        .map(|effect| effect.geometry(document, &geometry, border.width))
        .transpose()?;
    let mut marks = Marks::new(document, annotation);
    let path = [cloud
        .as_ref()
        .map_or_else(|| geometry.subpath(closed), cloud::Outline::path)];
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
    if let Some(endings) = endings
        && endings.iter().copied().any(ending::Style::visible)
    {
        ending::draw(
            document,
            annotation,
            &mut marks,
            endings,
            geometry.endpoints()?,
            &border,
        )?;
    }
    marks.finish(rect)
}
