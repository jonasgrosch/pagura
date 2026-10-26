//! Straight line annotations (12.5.6.7, Table 178).

use super::border::Border;
use super::marks::{Color, Marks, Pen, Subpath, color};
use super::polygon::no_endings;
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
    no_endings(document, annotation)?;
    no_leaders_or_caption(document, annotation)?;
    let border = Border::read(document, annotation)?;
    let mut marks = Marks::new(document, annotation);
    if border.width != 0.0
        && let Some(color) = color(document, annotation, b"C", Some(Color::black()))?
    {
        marks.stroke(
            &[Subpath::Polyline(&points)],
            &color,
            Pen {
                width: border.width,
                dash: &border.dash,
            },
        )?;
    }
    marks.finish(rect)
}

fn no_leaders_or_caption(document: &Document, annotation: &Dictionary) -> Result<()> {
    let leader = optional(document, annotation, b"LL")?;
    let extension = optional(document, annotation, b"LLE")?;
    if extension.is_some() && leader.is_none() {
        return Err(Error::syntax("line LLE requires LL"));
    }
    for (value, signed) in [
        (leader, true),
        (extension, false),
        (optional(document, annotation, b"LLO")?, false),
    ] {
        if let Some(value) = value {
            let value = number(document, value)?;
            if !signed && value < 0.0 {
                return Err(Error::syntax("line extension/offset must be nonnegative"));
            }
            if value != 0.0 {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "line leader generation is not implemented",
                ));
            }
        }
    }
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
