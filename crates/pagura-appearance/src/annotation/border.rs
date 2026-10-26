//! Link and square borders (12.5.4, 12.5.6.5, 12.5.6.8).

use super::marks::{Color, Marks, Pen, Subpath, color, scalar};
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::numbers;
use pagura_core::{Dictionary, Error, Name, Rect, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;

/// The annotation whose border is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// A link: a border only.
    Link,
    /// A square: an optional interior colour `IC` inside the border.
    Square,
}

/// The border inside `rect`, centred half a line width in, and for squares
/// the interior it encloses.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    shape: Shape,
) -> ReadResult<Option<AppearanceStream>> {
    if shape == Shape::Square && (annotation.contains_key(b"BE") || annotation.contains_key(b"RD"))
    {
        return Err(Error::unsupported(
            features::ANNOTATION_APPEARANCE,
            "square border effects and rectangle differences are not implemented",
        )
        .into());
    }
    let border = Border::read(document, annotation)?;
    let inset = (border.width * 0.5)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5);
    let inner = Rect::from_corners(
        rect.x0() + inset,
        rect.y0() + inset,
        rect.x1() - inset,
        rect.y1() - inset,
    )
    .ok_or(Error::syntax("invalid border rectangle"))?;
    let mut marks = Marks::new(document, annotation);
    let outline = Subpath::Rect {
        x: inner.x0(),
        y: inner.y0(),
        width: inner.width(),
        height: inner.height(),
    };
    if shape == Shape::Square
        && let Some(interior) = color(document, annotation, b"IC", None)?
    {
        marks.fill(&[outline], &interior)?;
    }
    if border.width != 0.0
        && let Some(color) = color(document, annotation, b"C", Some(Color::black()))?
    {
        let underline = [(inner.x0(), inner.y0()), (inner.x1(), inner.y0())];
        let path = if border.underline {
            Subpath::Polyline(&underline)
        } else {
            outline
        };
        let pen = Pen {
            width: border.width,
            dash: &border.dash,
        };
        marks.stroke(&[path], &color, pen)?;
    }
    Ok(marks.finish(rect)?)
}

/// An annotation border: BS, else Border, else solid and 1 unit wide
/// (Tables 166, 168).
pub(super) struct Border {
    pub(super) width: f64,
    pub(super) dash: Vec<f64>,
    pub(super) underline: bool,
}

impl Border {
    pub(super) fn read(document: &Document, annotation: &Dictionary) -> ReadResult<Self> {
        let (mut width, mut dash, mut style) = (1.0, Vec::new(), b"S".as_slice());
        if let Some(bs) = annotation.get(b"BS") {
            let bs = document.resolve_dictionary(bs)?;
            width = scalar(document, bs, b"W")?.unwrap_or(1.0);
            style = bs
                .get(b"S")
                .map(|value| document.resolve(value))
                .transpose()?
                .and_then(Value::as_name)
                .map_or(b"S", Name::as_bytes);
            if style == b"D" {
                dash = bs
                    .get(b"D")
                    .map(|value| numbers(document, value))
                    .transpose()?
                    .unwrap_or_else(|| vec![3.0]);
            }
        } else if let Some(border) = annotation.get(b"Border") {
            let border = document
                .resolve(border)?
                .as_array()
                .ok_or(Error::syntax("invalid annotation Border"))?;
            let [horizontal, vertical, line, rest @ ..] = border else {
                return Err(Error::syntax("invalid annotation Border").into());
            };
            if rest.len() > 1 {
                return Err(Error::syntax("invalid annotation Border").into());
            }
            let radii = [
                document.resolve(horizontal)?.as_number(),
                document.resolve(vertical)?.as_number(),
            ];
            if radii != [Some(0.0), Some(0.0)] {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "rounded annotation borders are not implemented",
                )
                .into());
            }
            width = document
                .resolve(line)?
                .as_number()
                .ok_or(Error::syntax("invalid annotation border width"))?;
            if let Some(array) = rest.first() {
                dash = numbers(document, array)?;
            }
        }
        if width < 0.0 {
            return Err(Error::syntax("negative annotation border width").into());
        }
        if matches!(style, b"B" | b"I") {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "relief annotation borders are not implemented",
            )
            .into());
        }
        Ok(Self {
            width,
            dash,
            underline: style == b"U",
        })
    }
}
