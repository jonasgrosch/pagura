//! Straight line annotations (12.5.6.7, Table 178).

mod leader;

use super::border::Border;
use super::ending::{self, Endpoint};
use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_document::Document;

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Option<AppearanceStream>> {
    let values = optional(document, annotation, b"L")?
        .and_then(Value::as_array)
        .ok_or(Error::syntax("line L must contain four numbers"))?;
    let [x0, y0, x1, y1] = values else {
        return Err(Error::syntax("line L must contain four numbers"));
    };
    let points = [
        (number(document, x0)?, number(document, y0)?),
        (number(document, x1)?, number(document, y1)?),
    ];
    let endings = ending::read(document, annotation)?;
    let layout = leader::Layout::read(document, annotation, points)?;
    no_caption(document, annotation)?;
    let border = Border::read(document, annotation)?;
    let mut marks = Marks::new(document, annotation);
    if border.width != 0.0
        && let Some(color) = color(document, annotation, b"C", Some(Color::black()))?
    {
        let mut paths = [
            Subpath::Polyline(&layout.line),
            Subpath::Polyline(&[]),
            Subpath::Polyline(&[]),
        ];
        if let Some([first, second]) = &layout.leaders {
            paths[1] = Subpath::Polyline(first);
            paths[2] = Subpath::Polyline(second);
        }
        marks.stroke(
            &paths,
            &color,
            Pen {
                width: border.width,
                dash: &border.dash,
            },
        )?;
    }
    if endings.iter().copied().any(ending::Style::visible) {
        ending::draw(
            document,
            annotation,
            &mut marks,
            endings,
            [
                Endpoint::new(layout.line[0], layout.line[1])?,
                Endpoint::new(layout.line[1], layout.line[0])?,
            ],
            &border,
        )?;
    }
    marks.finish(rect)
}

fn no_caption(document: &Document, annotation: &Dictionary) -> Result<()> {
    if let Some(value) = optional(document, annotation, b"Cap")? {
        let caption = value
            .as_bool()
            .ok_or(Error::syntax("line Cap must be boolean"))?;
        if caption {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "line caption generation is not implemented",
            ));
        }
    }
    Ok(())
}
