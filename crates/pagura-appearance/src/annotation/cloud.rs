//! Cloudy borders (12.5.4, Table 169), as ordinary bounded path artwork.
//!
//! The standard leaves lobe shape and spacing to the reader. Pagura uses
//! outward parabolic lobes, with diameter at most (4 + 4 I) max(W, 1).
//! The quadratic is represented exactly as a cubic. Intensity zero leaves
//! the original outline. BS width/dashes and annotation opacity still apply.

use super::marks::{PathOperator, Subpath};
use super::path::Geometry;
use crate::features;
use crate::values::{number, optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Meter, Rect, Result};
use pagura_document::Document;

type Point = (f64, f64);

pub(super) struct Effect {
    intensity: f64,
}

impl Effect {
    pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<Option<Self>> {
        let Some(effect) = optional_dictionary(document, annotation, b"BE")? else {
            return Ok(None);
        };
        let style = optional(document, effect, b"S")?
            .map(|value| {
                value
                    .as_name()
                    .ok_or(Error::syntax("border effect S must be a name"))
            })
            .transpose()?;
        match style.map_or(b"S".as_slice(), pagura_core::Name::as_bytes) {
            b"S" => Ok(None),
            b"C" => {
                let intensity = optional(document, effect, b"I")?
                    .map(|value| number(document, value))
                    .transpose()?
                    .unwrap_or(0.0);
                if !(0.0..=2.0).contains(&intensity) {
                    return Err(Error::syntax(
                        "cloud intensity must be between zero and two",
                    ));
                }
                Ok((intensity > 0.0).then_some(Self { intensity }))
            }
            _ => Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "unknown annotation border effect",
            )),
        }
    }

    pub(super) fn rectangle(
        &self,
        document: &Document,
        bounds: Rect,
        width: f64,
    ) -> Result<Outline> {
        Builder::new(document, self.diameter(width)?)?.polygon(&[
            (bounds.x0(), bounds.y0()),
            (bounds.x1(), bounds.y0()),
            (bounds.x1(), bounds.y1()),
            (bounds.x0(), bounds.y1()),
        ])
    }

    pub(super) fn ellipse(&self, document: &Document, bounds: Rect, width: f64) -> Result<Outline> {
        // Eight chords retain the ellipse's extrema; each chord is subdivided
        // before forming lobes. Curved border effects are reader artwork.
        const DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;
        const UNIT: [Point; 9] = [
            (1.0, 0.0),
            (DIAGONAL, DIAGONAL),
            (0.0, 1.0),
            (-DIAGONAL, DIAGONAL),
            (-1.0, 0.0),
            (-DIAGONAL, -DIAGONAL),
            (0.0, -1.0),
            (DIAGONAL, -DIAGONAL),
            (1.0, 0.0),
        ];
        let mut builder = Builder::new(document, self.diameter(width)?)?;
        let cx = bounds.x0().midpoint(bounds.x1());
        let cy = bounds.y0().midpoint(bounds.y1());
        let points = UNIT.map(|(x, y)| {
            (
                cx + x * bounds.width() / 2.0,
                cy + y * bounds.height() / 2.0,
            )
        });
        for pair in points.windows(2) {
            builder.edge(pair[0], pair[1], 1.0)?;
        }
        Ok(builder.finish())
    }

    pub(super) fn geometry(
        &self,
        document: &Document,
        geometry: &Geometry,
        width: f64,
    ) -> Result<Outline> {
        let mut builder = Builder::new(document, self.diameter(width)?)?;
        match geometry {
            Geometry::Vertices(points) => builder.polygon(points),
            Geometry::Operators(commands) => {
                // Path supersedes Vertices, including curves. Sampling the source
                // curve is a border-effect design choice, never a renderer path.
                let mut points = Vec::new();
                let mut current = (0.0, 0.0);
                for command in commands {
                    builder.work.charge(1)?;
                    match *command {
                        PathOperator::Move([x, y]) | PathOperator::Line([x, y]) => {
                            builder.storage.reserve_capacity(&mut points, 1)?;
                            current = (x, y);
                            points.push(current);
                        }
                        PathOperator::Cubic([x1, y1, x2, y2, x3, y3]) => {
                            builder.work.charge(8)?;
                            builder.storage.reserve_capacity(&mut points, 8)?;
                            for index in 1..=8 {
                                let t = f64::from(index) / 8.0;
                                let u = 1.0 - t;
                                points.push((
                                    u.powi(3) * current.0
                                        + 3.0 * u * u * t * x1
                                        + 3.0 * u * t * t * x2
                                        + t.powi(3) * x3,
                                    u.powi(3) * current.1
                                        + 3.0 * u * u * t * y1
                                        + 3.0 * u * t * t * y2
                                        + t.powi(3) * y3,
                                ));
                            }
                            current = (x3, y3);
                        }
                    }
                }
                builder.polygon(&points)
            }
        }
    }

    fn diameter(&self, width: f64) -> Result<f64> {
        let diameter = (4.0 + 4.0 * self.intensity) * width.max(1.0);
        if !diameter.is_finite() {
            return Err(Error::limit("cloud diameter exceeds numeric range"));
        }
        Ok(diameter)
    }
}

pub(super) struct Outline(Vec<PathOperator>);

impl Outline {
    pub(super) fn path(&self) -> Subpath<'_> {
        Subpath::Operators {
            commands: &self.0,
            closed: true,
        }
    }
}

struct Builder {
    commands: Vec<PathOperator>,
    work: Meter,
    storage: Meter,
    geometry: Meter,
    diameter: f64,
}

impl Builder {
    fn new(document: &Document, diameter: f64) -> Result<Self> {
        let limits = document.limits();
        let mut geometry = Meter::new(limits.path_segments, "cloud geometry budget exceeded");
        geometry.charge(1)?;
        Ok(Self {
            commands: Vec::new(),
            diameter,
            work: Meter::new(limits.operations, "cloud work budget exceeded"),
            storage: Meter::new(limits.document_bytes, "cloud storage budget exceeded"),
            geometry,
        })
    }

    fn edge(&mut self, start: Point, end: Point, winding: f64) -> Result<()> {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let length = dx.hypot(dy);
        let count = (length / self.diameter).ceil().max(1.0);
        if !count.is_finite() || count > self.geometry.remaining() as f64 {
            return Err(Error::limit("cloud geometry budget exceeded"));
        }
        let count = count_to_usize(count);
        self.geometry.charge(count)?;
        self.work.charge(count)?;
        self.storage
            .reserve_capacity(&mut self.commands, count.saturating_add(1))?;
        if self.commands.is_empty() {
            self.commands.push(PathOperator::Move([start.0, start.1]));
        }
        let step = (dx / count as f64, dy / count as f64);
        for index in 0..count {
            let from = (
                start.0 + step.0 * index as f64,
                start.1 + step.1 * index as f64,
            );
            let to = if index + 1 == count {
                end
            } else {
                (from.0 + step.0, from.1 + step.1)
            };
            let normal = (step.1 * winding * 2.0 / 3.0, -step.0 * winding * 2.0 / 3.0);
            let operands = [
                from.0 + step.0 / 3.0 + normal.0,
                from.1 + step.1 / 3.0 + normal.1,
                to.0 - step.0 / 3.0 + normal.0,
                to.1 - step.1 / 3.0 + normal.1,
                to.0,
                to.1,
            ];
            if operands.iter().any(|value| !value.is_finite()) {
                return Err(Error::limit("cloud controls exceed numeric range"));
            }
            self.commands.push(PathOperator::Cubic(operands));
        }
        Ok(())
    }

    fn polygon(mut self, points: &[Point]) -> Result<Outline> {
        let Some(&origin) = points.first() else {
            return Ok(self.finish());
        };
        let mut area = 0.0;
        let mut last = origin;
        for &point in points.iter().skip(1) {
            self.work.charge(1)?;
            area += (last.0 - origin.0) * (point.1 - origin.1)
                - (last.1 - origin.1) * (point.0 - origin.0);
            last = point;
        }
        if !area.is_finite() {
            return Err(Error::limit("cloud orientation exceeds numeric range"));
        }
        let winding = if area < 0.0 { -1.0 } else { 1.0 };
        last = origin;
        for &point in points.iter().skip(1).chain(std::iter::once(&origin)) {
            self.edge(last, point, winding)?;
            last = point;
        }
        Ok(self.finish())
    }

    fn finish(self) -> Outline {
        Outline(self.commands)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "positive integral count checked against the usize path budget"
)]
fn count_to_usize(count: f64) -> usize {
    count as usize
}
