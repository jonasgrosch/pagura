//! Finite convex text-markup quadrilaterals (Table 182).

use crate::repairs;
use crate::values::{number, optional};
use pagura_core::{Diagnostic, Dictionary, Error, Meter, Result, Value};
use pagura_document::Document;

pub(super) type Point = (f64, f64);

pub(super) struct Quad(pub(super) [Point; 4]);

impl Quad {
    pub(super) fn at(&self, u: f64, v: f64) -> Result<Point> {
        let [a, b, c, d] = self.0;
        let bottom = lerp(a, b, u);
        let top = lerp(d, c, u);
        let point = lerp(bottom, top, v);
        if !point.0.is_finite() || !point.1.is_finite() {
            return Err(Error::limit("markup geometry exceeds numeric range"));
        }
        Ok(point)
    }

    pub(super) fn extent(&self) -> Result<Point> {
        let [a, b, c, d] = self.0;
        let distance = |a: Point, b: Point| (a.0 - b.0).hypot(a.1 - b.1);
        let width = distance(a, b) / 2.0 + distance(d, c) / 2.0;
        let height = distance(a, d) / 2.0 + distance(b, c) / 2.0;
        if !width.is_finite() || !height.is_finite() {
            return Err(Error::limit("markup extent exceeds numeric range"));
        }
        Ok((width, height))
    }
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    (a.0 * (1.0 - t) + b.0 * t, a.1 * (1.0 - t) + b.1 * t)
}

pub(super) fn read(
    document: &Document,
    annotation: &Dictionary,
    storage: &mut Meter,
) -> Result<Vec<Quad>> {
    let values = optional(document, annotation, b"QuadPoints")?
        .and_then(Value::as_array)
        .ok_or(Error::syntax("markup QuadPoints must be an array"))?;
    let (groups, tail) = values.as_chunks::<8>();
    if groups.is_empty() || !tail.is_empty() {
        return Err(Error::syntax(
            "markup QuadPoints requires groups of eight numbers",
        ));
    }
    if groups.len() > document.limits().path_segments / 4
        || groups.len() > document.limits().traversal_work / 8
    {
        return Err(Error::limit("markup quad count exceeded"));
    }
    let mut quads = Vec::new();
    storage.reserve(&mut quads, groups.len())?;
    for [x0, y0, x1, y1, x2, y2, x3, y3] in groups {
        let points = [
            (number(document, x0)?, number(document, y0)?),
            (number(document, x1)?, number(document, y1)?),
            (number(document, x2)?, number(document, y2)?),
            (number(document, x3)?, number(document, y3)?),
        ];
        let points = if counterclockwise(&points)? {
            points
        } else {
            let [top_left, top_right, bottom_left, bottom_right] = points;
            let reordered = [bottom_left, bottom_right, top_right, top_left];
            if !counterclockwise(&reordered)? {
                return Err(Error::syntax(
                    "markup QuadPoints is not a convex quadrilateral",
                ));
            }
            document.repair(Diagnostic::new(
                repairs::QUAD_ORDER,
                "text markup QuadPoints reordered to counterclockwise vertices (Table 182)",
            ))?;
            reordered
        };
        quads.push(Quad(points));
    }
    Ok(quads)
}

fn counterclockwise(points: &[Point; 4]) -> Result<bool> {
    let [a, b, c, d] = *points;
    let mut valid = true;
    for (a, b, c) in [(a, b, c), (b, c, d), (c, d, a), (d, a, b)] {
        let cross = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
        if !cross.is_finite() {
            return Err(Error::limit("markup orientation exceeds numeric range"));
        }
        valid &= cross >= 0.0;
    }
    Ok(valid)
}
