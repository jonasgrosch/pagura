//! Link, square and circle borders (12.5.4, 12.5.6.5, 12.5.6.8).

mod form;
mod relief;

use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::features;
use crate::stream::AppearanceStream;
use crate::values::{number, numbers, optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Meter, Name, Rect, Result};
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
    let border = if shape == Shape::Circle {
        Border::read(document, annotation)?
    } else {
        Border::read_rectangular(document, annotation)?
    };
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
        border.outline(inner)
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
        if !border.paint_relief(&mut marks, bounds, &color)? {
            marks.stroke(&[path], &color, pen)?;
        }
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
pub(crate) struct Border {
    pub(crate) width: f64,
    pub(super) dash: Vec<f64>,
    pub(super) underline: bool,
    radii: [f64; 2],
    relief: Option<relief::Kind>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Corners {
    Square,
    Rounded,
}

impl Border {
    /// Styles for paths whose own geometry does not consume corner radii.
    pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<Self> {
        Self::read_with_corners(document, annotation, Corners::Square)
    }

    /// Styles for a rectangular border, including legacy Border corner radii.
    pub(crate) fn read_rectangular(document: &Document, annotation: &Dictionary) -> Result<Self> {
        Self::read_with_corners(document, annotation, Corners::Rounded)
    }

    /// The border centreline; radii are clamped to half each extent when written.
    pub(super) fn outline(&self, bounds: Rect) -> Subpath<'static> {
        if self.radii.contains(&0.0) {
            Subpath::Rect {
                x: bounds.x0(),
                y: bounds.y0(),
                width: bounds.width(),
                height: bounds.height(),
            }
        } else {
            Subpath::RoundedRect {
                bounds,
                radii: self.radii,
            }
        }
    }

    /// Paints B/I facets and returns whether a relief style was selected.
    pub(super) fn paint_relief(
        &self,
        marks: &mut Marks<'_>,
        bounds: Rect,
        color: &Color,
    ) -> Result<bool> {
        let Some(kind) = self.relief else {
            return Ok(false);
        };
        relief::paint(marks, bounds, self.width, color, kind)?;
        Ok(true)
    }

    /// Worst-case number of edges in one rectangular outline.
    pub(super) fn outline_segments(&self) -> usize {
        if self.radii.contains(&0.0) && self.relief.is_none() {
            4
        } else {
            12
        }
    }

    fn read_with_corners(
        document: &Document,
        annotation: &Dictionary,
        corners: Corners,
    ) -> Result<Self> {
        let mut radii = [0.0; 2];
        let (mut width, mut dash, mut style) = (1.0, Vec::new(), b"S".as_slice());
        if let Some(bs) = optional_dictionary(document, annotation, b"BS")? {
            width = optional(document, bs, b"W")?
                .map(|value| number(document, value))
                .transpose()?
                .unwrap_or(1.0);
            style = optional(document, bs, b"S")?
                .map(|value| value.as_name().ok_or(Error::syntax("BS S must be a name")))
                .transpose()?
                .map_or(b"S", Name::as_bytes);
            if style == b"D" {
                dash = optional(document, bs, b"D")?
                    .map(|value| numbers(document, value))
                    .transpose()?
                    .unwrap_or_else(|| vec![3.0]);
            }
        } else if let Some(border) = optional(document, annotation, b"Border")? {
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
            radii = [number(document, horizontal)?, number(document, vertical)?];
            if radii.iter().any(|radius| *radius < 0.0) {
                return Err(Error::syntax("negative annotation corner radius"));
            }
            if corners == Corners::Square && radii != [0.0, 0.0] {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "rounded borders require rectangular annotation geometry",
                ));
            }
            width = number(document, line)?;
            if let Some(array) = rest.first() {
                dash = numbers(document, array)?;
            }
        }
        if width < 0.0 {
            return Err(Error::syntax("negative annotation border width"));
        }
        if dash.iter().any(|value| *value < 0.0)
            || !dash.is_empty() && dash.iter().all(|value| *value == 0.0)
        {
            return Err(Error::syntax("invalid annotation dash pattern"));
        }
        let relief = match style {
            b"B" => Some(relief::Kind::Raised),
            b"I" => Some(relief::Kind::Sunken),
            _ => None,
        };
        if corners == Corners::Square && relief.is_some() {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "relief borders require rectangular annotation geometry",
            ));
        }
        if relief.is_some() {
            let limits = document.limits();
            Meter::new(
                limits.path_segments,
                "relief border geometry budget exceeded",
            )
            .charge(16)?; // Optional rectangular fill and both six-edge facets.
            Meter::new(limits.operations, "relief border work budget exceeded").charge(24)?;
        }
        if !radii.contains(&0.0) {
            // At most a fill and a stroke, each with eight arcs and four edges.
            let limits = document.limits();
            Meter::new(
                limits.path_segments,
                "rounded border geometry budget exceeded",
            )
            .charge(24)?;
            Meter::new(limits.operations, "rounded border work budget exceeded").charge(24)?;
        }
        Ok(Self {
            relief,
            radii,
            width,
            dash,
            underline: style == b"U",
        })
    }
}
