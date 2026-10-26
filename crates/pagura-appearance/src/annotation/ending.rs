//! Line and PolyLine ending artwork (12.5.6.7, Table 179).
//!
//! Dimensions are reader policy: radius/half-height 3*max(width, 1), arrow
//! depth twice that, and solid ending outlines. Body, interior and outline
//! paint in that order, with the existing independent CA/ca semantics.

use super::border::Border;
use super::marks::{Color, Marks, Pen, Subpath, color};
use crate::features;
use crate::values::optional;
use pagura_core::{Dictionary, Error, Rect, Result};
use pagura_document::Document;

type Point = (f64, f64);

#[derive(Clone, Copy)]
pub(super) struct Endpoint {
    point: Point,
    outward: Point,
}

impl Endpoint {
    /// The endpoint faces away from the adjacent source point/control.
    /// A directionless point uses the positive x axis as reader policy.
    pub(super) fn new(point: Point, adjacent: Point) -> Result<Self> {
        let delta = (point.0 - adjacent.0, point.1 - adjacent.1);
        let length = delta.0.hypot(delta.1);
        if !length.is_finite() {
            return Err(Error::limit("line ending direction exceeds numeric range"));
        }
        let outward = if length == 0.0 {
            (1.0, 0.0)
        } else {
            (delta.0 / length, delta.1 / length)
        };
        Ok(Self { point, outward })
    }

    fn map(self, (x, y): Point) -> Result<Point> {
        let (a, b) = self.outward;
        let point = (self.point.0 + a * x - b * y, self.point.1 + b * x + a * y);
        if !point.0.is_finite() || !point.1.is_finite() {
            return Err(Error::limit("line ending exceeds numeric range"));
        }
        Ok(point)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Style {
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ReverseOpenArrow,
    ReverseClosedArrow,
    Slash,
}

pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<[Style; 2]> {
    let Some(value) = optional(document, annotation, b"LE")? else {
        return Ok([Style::None; 2]);
    };
    let values = value
        .as_array()
        .ok_or(Error::syntax("line endings must be an array"))?;
    let [start, end] = values else {
        return Err(Error::syntax("line endings require two names"));
    };
    let parse = |value| {
        let name = document
            .resolve(value)?
            .as_name()
            .ok_or(Error::syntax("line ending must be a name"))?;
        match name.as_bytes() {
            b"None" => Ok(Style::None),
            b"Square" => Ok(Style::Square),
            b"Circle" => Ok(Style::Circle),
            b"Diamond" => Ok(Style::Diamond),
            b"OpenArrow" => Ok(Style::OpenArrow),
            b"ClosedArrow" => Ok(Style::ClosedArrow),
            b"Butt" => Ok(Style::Butt),
            b"ROpenArrow" => Ok(Style::ReverseOpenArrow),
            b"RClosedArrow" => Ok(Style::ReverseClosedArrow),
            b"Slash" => Ok(Style::Slash),
            _ => Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "unknown line ending style",
            )),
        }
    };
    Ok([parse(start)?, parse(end)?])
}

enum Shape {
    Line([Point; 2]),
    Open([Point; 3]),
    Triangle([Point; 3]),
    Quadrilateral([Point; 4]),
    Circle(Rect),
}

impl Shape {
    fn subpath(&self) -> Subpath<'_> {
        match self {
            Self::Line(points) => Subpath::Polyline(points),
            Self::Open(points) => Subpath::Polyline(points),
            Self::Triangle(points) => Subpath::Polygon(points),
            Self::Quadrilateral(points) => Subpath::Polygon(points),
            Self::Circle(bounds) => Subpath::Ellipse(*bounds),
        }
    }

    fn filled(&self) -> bool {
        matches!(
            self,
            Self::Triangle(_) | Self::Quadrilateral(_) | Self::Circle(_)
        )
    }
}

impl Style {
    pub(super) fn visible(self) -> bool {
        !matches!(self, Self::None)
    }

    fn shape(self, endpoint: Endpoint, width: f64) -> Result<Option<Shape>> {
        if !self.visible() {
            return Ok(None);
        }
        let r = 3.0 * width.max(1.0);
        if !r.is_finite() {
            return Err(Error::limit("line ending size exceeds numeric range"));
        }
        let point = |x, y| endpoint.map((x, y));
        let shape = match self {
            Self::None => return Ok(None),
            Self::Square => {
                Shape::Quadrilateral([point(-r, -r)?, point(r, -r)?, point(r, r)?, point(-r, r)?])
            }
            Self::Diamond => Shape::Quadrilateral([
                point(-r, 0.0)?,
                point(0.0, -r)?,
                point(r, 0.0)?,
                point(0.0, r)?,
            ]),
            Self::Circle => {
                let (x, y) = endpoint.point;
                let bounds = Rect::from_corners(x - r, y - r, x + r, y + r)
                    .ok_or(Error::limit("line ending circle exceeds numeric range"))?;
                Shape::Circle(bounds)
            }
            Self::Butt => Shape::Line([point(0.0, -r)?, point(0.0, r)?]),
            Self::Slash => {
                let y = r * 3.0_f64.sqrt() * 0.5;
                Shape::Line([point(-r * 0.5, -y)?, point(r * 0.5, y)?])
            }
            Self::OpenArrow
            | Self::ClosedArrow
            | Self::ReverseOpenArrow
            | Self::ReverseClosedArrow => {
                let x = if matches!(self, Self::ReverseOpenArrow | Self::ReverseClosedArrow) {
                    2.0 * r
                } else {
                    -2.0 * r
                };
                let points = [point(x, r)?, point(0.0, 0.0)?, point(x, -r)?];
                if matches!(self, Self::ClosedArrow | Self::ReverseClosedArrow) {
                    Shape::Triangle(points)
                } else {
                    Shape::Open(points)
                }
            }
        };
        Ok(Some(shape))
    }
}

pub(super) fn draw(
    document: &Document,
    annotation: &Dictionary,
    marks: &mut Marks<'_>,
    styles: [Style; 2],
    endpoints: [Endpoint; 2],
    border: &Border,
) -> Result<()> {
    let shapes = [
        styles[0].shape(endpoints[0], border.width)?,
        styles[1].shape(endpoints[1], border.width)?,
    ];
    let mut outlines = [Subpath::Polyline(&[]); 2];
    let mut interiors = [Subpath::Polyline(&[]); 2];
    let mut filled = false;
    let mut present = false;
    for ((shape, outline), interior) in shapes.iter().zip(&mut outlines).zip(&mut interiors) {
        if let Some(shape) = shape {
            *outline = shape.subpath();
            present = true;
            if shape.filled() {
                *interior = shape.subpath();
                filled = true;
            }
        }
    }
    if filled && let Some(color) = color(document, annotation, b"IC", None)? {
        marks.fill(&interiors, &color)?;
    }
    if present
        && border.width != 0.0
        && let Some(color) = color(document, annotation, b"C", Some(Color::black()))?
    {
        marks.stroke(
            &outlines,
            &color,
            Pen {
                width: border.width,
                dash: &[],
            },
        )?;
    }
    Ok(())
}
