//! Link, square and circle borders (12.5.4, 12.5.6.5, 12.5.6.8).

use super::marks::{Color, Marks, Pen, Subpath, color, scalar};
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::{number, numbers, optional};
use pagura_core::{Dictionary, Error, Name, Rect, Result, Value};
use pagura_document::Document;

/// The annotation whose border is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// A link: a border only.
    Link,
    /// A square: an optional interior colour `IC` inside the border.
    Square,
    /// A circle/ellipse with optional interior colour `IC`.
    Circle,
}

/// The border inside `rect`, centred half a line width in, and for squares
/// and circles the interior it encloses. Rectangle differences reduce the shape bounds
/// before border inset; the appearance BBox stays the original annotation Rect.
pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    shape: Shape,
) -> Result<Option<AppearanceStream>> {
    let bounds = if shape == Shape::Link {
        rect
    } else {
        plain_effect(document, annotation)?;
        inset_bounds(document, annotation, rect)?
    };
    let border = Border::read(document, annotation)?;
    let inset = (border.width * 0.5)
        .min(bounds.width() * 0.5)
        .min(bounds.height() * 0.5);
    let inner = Rect::from_corners(
        bounds.x0() + inset,
        bounds.y0() + inset,
        bounds.x1() - inset,
        bounds.y1() - inset,
    )
    .ok_or(Error::syntax("invalid border rectangle"))?;
    let mut marks = Marks::new(document, annotation);
    let outline = if shape == Shape::Circle {
        Subpath::Ellipse(inner)
    } else {
        Subpath::Rect {
            x: inner.x0(),
            y: inner.y0(),
            width: inner.width(),
            height: inner.height(),
        }
    };
    if shape != Shape::Link
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
    marks.finish(rect)
}

/// Tables 180/183 specify RD in left, top, right, bottom order, unlike a Rect.
pub(super) fn inset_bounds(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
) -> Result<Rect> {
    if !rect.width().is_finite() || !rect.height().is_finite() {
        return Err(Error::limit("annotation extent exceeds numeric range"));
    }
    let Some(differences) = optional(document, annotation, b"RD")? else {
        return Ok(rect);
    };
    let differences = differences
        .as_array()
        .ok_or(Error::syntax("annotation RD must be an array"))?;
    let [left, top, right, bottom] = differences else {
        return Err(Error::syntax("annotation RD must have four numbers"));
    };
    let [left, top, right, bottom] = [
        number(document, left)?,
        number(document, top)?,
        number(document, right)?,
        number(document, bottom)?,
    ];
    let horizontal = left + right;
    let vertical = top + bottom;
    if !horizontal.is_finite() || !vertical.is_finite() {
        return Err(Error::limit("annotation insets exceed numeric range"));
    }
    if [left, top, right, bottom].iter().any(|value| *value < 0.0)
        || horizontal >= rect.width()
        || vertical >= rect.height()
    {
        return Err(Error::syntax("annotation RD leaves invalid shape bounds"));
    }
    Rect::from_corners(
        rect.x0() + left,
        rect.y0() + bottom,
        rect.x1() - right,
        rect.y1() - top,
    )
    .ok_or(Error::syntax("invalid annotation shape bounds"))
}

/// Border effects apply to Square/Circle and Polygon (Tables 180/181).
pub(super) fn plain_effect(document: &Document, annotation: &Dictionary) -> Result<()> {
    if let Some(effect) = optional(document, annotation, b"BE")? {
        let effect = effect
            .as_dictionary()
            .ok_or(Error::syntax("annotation BE must be a dictionary"))?;
        if let Some(style) = optional(document, effect, b"S")? {
            let style = style
                .as_name()
                .ok_or(Error::syntax("border effect S must be a name"))?;
            if style != b"S" {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "cloudy or unrecognized annotation border effect",
                ));
            }
        }
    }
    Ok(())
}

/// An annotation border: BS, else Border, else solid and 1 unit wide
/// (Tables 166, 168).
pub(super) struct Border {
    pub(super) width: f64,
    pub(super) dash: Vec<f64>,
    pub(super) underline: bool,
}

impl Border {
    pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<Self> {
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
                return Err(Error::syntax("invalid annotation Border"));
            };
            if rest.len() > 1 {
                return Err(Error::syntax("invalid annotation Border"));
            }
            let radii = [
                document.resolve(horizontal)?.as_number(),
                document.resolve(vertical)?.as_number(),
            ];
            if radii != [Some(0.0), Some(0.0)] {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "rounded annotation borders are not implemented",
                ));
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
            return Err(Error::syntax("negative annotation border width"));
        }
        if matches!(style, b"B" | b"I") {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "relief annotation borders are not implemented",
            ));
        }
        Ok(Self {
            width,
            dash,
            underline: style == b"U",
        })
    }
}
