//! Plain Line captions, Table 178 and Figures 81–82 (12.5.6.7).
//!
//! Reader policy uses 12-point Helvetica, 1.2-em leading and two units of
//! clearance. Lines follow L's direction. Inline text interrupts the body
//! only where its box overlaps it; text wider than the line goes above it.

mod text;

use crate::features;
use crate::fonts::FontSource;
use crate::form::da::DefaultAppearance;
use crate::program::Program;
use crate::rich;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::{Document, forms::Justification};
use text::Text;

type Point = (f64, f64);
const SIZE: f64 = 12.0;
const LEADING: f64 = SIZE * 1.2;
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
        if optional(document, annotation, b"RC")?.is_some() {
            let appearance = DefaultAppearance::parse(b"/Helvetica 12 Tf", &document.limits())?;
            let rich = rich::prepare(
                document,
                annotation,
                "",
                &appearance,
                Justification::Left,
                f64::INFINITY,
                fonts,
            )?
            .ok_or(Error::syntax("missing rich caption content"))?;
            if rich.fonts.is_empty() {
                return Ok(None);
            }
            let layout = Layout::with_extent(
                line,
                position,
                offset,
                Extent {
                    width: rich.width,
                    height: rich.height,
                    ascent: 0.0,
                },
            )?;
            let origin = shifted(
                layout.center,
                layout.axis,
                -rich.width / 2.0,
                layout.baseline,
            )?;
            let mut out = Program::new(&document.limits());
            rich.write(&mut out, origin, layout.axis)?;
            return Ok(Some(Self {
                body: layout.body,
                content: out.finish(),
                fonts: rich.fonts,
            }));
        }
        let Some(text) = Text::read(document, annotation, fonts)? else {
            return Ok(None);
        };
        let layout = Layout::new(line, position, offset, &text)?;
        let content = layout.write(document, &text)?;
        Ok(Some(Self {
            body: layout.body,
            content,
            fonts: text.fonts,
        }))
    }
}

struct Layout {
    body: [[Point; 2]; 2],
    axis: Point,
    center: Point,
    baseline: f64,
}

struct Extent {
    width: f64,
    height: f64,
    ascent: f64,
}

impl Layout {
    fn new(line: [Point; 2], position: Position, offset: Point, text: &Text) -> Result<Self> {
        let width = text
            .rows
            .iter()
            .map(|row| row.width * SIZE)
            .fold(0.0, f64::max);
        let height = (text.ascent - text.descent) * SIZE
            + text.rows.len().saturating_sub(1) as f64 * LEADING;
        Self::with_extent(
            line,
            position,
            offset,
            Extent {
                width,
                height,
                ascent: text.ascent * SIZE,
            },
        )
    }

    fn with_extent(
        line: [Point; 2],
        position: Position,
        offset: Point,
        extent: Extent,
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
        let Extent {
            width,
            height,
            ascent,
        } = extent;
        let top = position == Position::Top || width + 2.0 * CLEARANCE > length;
        let baseline = if top {
            height + CLEARANCE
        } else {
            height / 2.0
        } - ascent;
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
        if !width.is_finite() || !height.is_finite() || !baseline.is_finite() {
            return Err(Error::limit("caption size exceeds numeric range"));
        }
        Ok(Self {
            body,
            axis,
            center,
            baseline,
        })
    }

    fn write(&self, document: &Document, text: &Text) -> Result<Vec<u8>> {
        let mut out = Program::new(&document.limits());
        out.text("BT\n0 Tc\n0 Tw\n100 Tz\n0 TL\n0 Tr\n0 Ts\n/Caption 12 Tf\n")?;
        let (x, y) = self.axis;
        for (index, row) in text.rows.iter().enumerate() {
            let (tx, ty) = shifted(
                self.center,
                self.axis,
                -row.width * SIZE / 2.0,
                self.baseline - index as f64 * LEADING,
            )?;
            out.text(&format!("{x} {y} {} {x} {tx} {ty} Tm\n<", -y))?;
            for code in text
                .codes
                .get(row.codes.clone())
                .ok_or(Error::syntax("invalid caption row"))?
            {
                out.text(&format!("{code:02X}"))?;
            }
            out.text("> Tj\n")?;
        }
        out.text("ET\n")?;
        Ok(out.finish())
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
