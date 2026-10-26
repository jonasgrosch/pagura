//! Plain and rich Line captions, Table 178 and Figures 81–82 (12.5.6.7).
//!
//! Reader policy uses 12-point Helvetica, 1.2-em leading and two units of
//! clearance. Lines follow L's direction. Inline text interrupts the body
//! only where its box overlaps it; text wider than the line goes above it.

use crate::features;
use crate::fonts::FontSource;
use crate::form::da::DefaultAppearance;
use crate::program::Program;
use crate::rich;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Meter, Result};
use pagura_document::{Document, forms::Justification};

type Point = (f64, f64);
const CLEARANCE: f64 = 2.0;

pub(super) struct Caption {
    pub(super) body: [[Point; 2]; 2],
    pub(super) content: Vec<u8>,
    pub(super) fonts: Dictionary,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Inline,
    Top,
}

impl Caption {
    pub(super) fn read(
        document: &Document,
        annotation: &Dictionary,
        line: [Point; 2],
        fonts: &dyn FontSource,
    ) -> Result<Option<Self>> {
        let enabled = optional(document, annotation, b"Cap")?
            .map(|v| v.as_bool().ok_or(Error::syntax("line Cap must be boolean")))
            .transpose()?
            .unwrap_or(false);
        if !enabled {
            return Ok(None);
        }
        let position = position(document, annotation)?;
        let offset = offset(document, annotation)?;
        let appearance = DefaultAppearance::parse(b"/Helvetica 12 Tf", &document.limits())?;
        let mut storage = Meter::new(
            document.limits().document_bytes,
            "caption storage budget exceeded",
        );
        let text = rich::prepare(
            document,
            annotation,
            &appearance,
            Justification::Center,
            f64::INFINITY,
            fonts,
            &mut storage,
        )?;
        if text.fonts.is_empty() {
            return Ok(None);
        }
        let layout = Layout::new(line, position, offset, (text.width, text.height))?;
        let origin = shifted(layout.center, layout.axis, -text.width / 2.0, layout.top)?;
        let mut out = Program::new(&document.limits());
        text.write(&mut out, origin, layout.axis)?;
        Ok(Some(Self {
            body: layout.body,
            content: out.finish(),
            fonts: text.fonts,
        }))
    }
}

struct Layout {
    body: [[Point; 2]; 2],
    axis: Point,
    center: Point,
    top: f64,
}

impl Layout {
    fn new(
        line: [Point; 2],
        position: Position,
        offset: Point,
        extent: (f64, f64),
    ) -> Result<Self> {
        let dx = line[1].0 - line[0].0;
        let dy = line[1].1 - line[0].1;
        let length = dx.hypot(dy);
        if !length.is_finite() {
            return Err(Error::limit("caption direction exceeds numeric range"));
        }
        let axis = if length == 0.0 {
            (1.0, 0.0)
        } else {
            (dx / length, dy / length)
        };
        let (width, height) = extent;
        let top = position == Position::Top || width + 2.0 * CLEARANCE > length;
        let text_top = if top {
            height + CLEARANCE
        } else {
            height / 2.0
        };
        let midpoint = (
            line[0].0 / 2.0 + line[1].0 / 2.0,
            line[0].1 / 2.0 + line[1].1 / 2.0,
        );
        let center = shifted(midpoint, axis, offset.0, offset.1)?;
        let mut body = [line, [line[1]; 2]];
        if !top && offset.1.abs() < height / 2.0 + CLEARANCE {
            let middle = length / 2.0 + offset.0;
            if !middle.is_finite() {
                return Err(Error::limit("caption gap exceeds numeric range"));
            }
            let before = (middle - width / 2.0 - CLEARANCE).clamp(0.0, length);
            let after = (middle + width / 2.0 + CLEARANCE).clamp(0.0, length);
            body = [
                [line[0], shifted(line[0], axis, before, 0.0)?],
                [shifted(line[0], axis, after, 0.0)?, line[1]],
            ];
        }
        if !width.is_finite() || !height.is_finite() || !text_top.is_finite() {
            return Err(Error::limit("caption size exceeds numeric range"));
        }
        Ok(Self {
            body,
            axis,
            center,
            top: text_top,
        })
    }
}

fn shifted(point: Point, axis: Point, x: f64, y: f64) -> Result<Point> {
    let point = (
        point.0 + axis.0 * x - axis.1 * y,
        point.1 + axis.1 * x + axis.0 * y,
    );
    if !point.0.is_finite() || !point.1.is_finite() {
        return Err(Error::limit("caption position exceeds numeric range"));
    }
    Ok(point)
}

fn position(document: &Document, annotation: &Dictionary) -> Result<Position> {
    let Some(value) = optional(document, annotation, b"CP")? else {
        return Ok(Position::Inline);
    };
    match value
        .as_name()
        .ok_or(Error::syntax("line CP must be a name"))?
        .as_bytes()
    {
        b"Inline" => Ok(Position::Inline),
        b"Top" => Ok(Position::Top),
        _ => Err(Error::unsupported(
            features::ANNOTATION_APPEARANCE,
            "unknown line caption position",
        )),
    }
}

fn offset(document: &Document, annotation: &Dictionary) -> Result<Point> {
    let Some(value) = optional(document, annotation, b"CO")? else {
        return Ok((0.0, 0.0));
    };
    let [x, y] = value
        .as_array()
        .ok_or(Error::syntax("line CO must be two numbers"))?
    else {
        return Err(Error::syntax("line CO must be two numbers"));
    };
    Ok((number(document, x)?, number(document, y)?))
}
