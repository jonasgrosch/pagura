//! Text-markup appearances (12.5.6.10, Table 182).
//!
//! Artwork policy fills the quads for Highlight with Multiply blending;
//! Underline is the bottom 1/12 of each quad and StrikeOut a centred 1/12
//! band. Squiggly is a 1/24-height ribbon with peaks 1/12 apart vertically
//! and horizontal steps no longer than 1/6 of the quad height. All regions
//! share one nonzero fill, so overlapping quads do not apply opacity twice.

mod quads;

use super::marks::{Color, Marks, PathOperator, Subpath, color};
use crate::stream::AppearanceStream;
use pagura_core::{Dictionary, Error, Meter, Rect, Result};
use pagura_document::Document;
use quads::Quad;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

pub(super) fn generate(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    style: Style,
) -> Result<Option<AppearanceStream>> {
    let limits = document.limits();
    let mut storage = Meter::new(limits.render_bytes, "markup geometry storage exceeded");
    let quads = quads::read(document, annotation, &mut storage)?;
    let mut segments = Meter::new(limits.path_segments, "markup path segment budget exceeded");
    let mut work = Meter::new(limits.operations, "markup path work budget exceeded");
    let mut commands = Vec::new();
    for quad in &quads {
        let count = if style == Style::Squiggly {
            steps(quad, &limits)?
        } else {
            1
        };
        let needed = count
            .checked_add(1)
            .and_then(|n| n.checked_mul(2))
            .ok_or(Error::limit("markup path size overflow"))?;
        segments.charge(needed)?;
        work.charge(needed)?;
        storage.reserve(&mut commands, needed)?;
        append(&mut commands, quad, style, count)?;
    }
    let default = if style == Style::Highlight {
        Color::yellow()
    } else {
        Color::black()
    };
    let mut marks = Marks::new(document, annotation);
    if let Some(color) = color(document, annotation, b"C", Some(default))? {
        if style == Style::Highlight {
            marks.multiply()?;
        }
        marks.fill(
            &[Subpath::Operators {
                commands: &commands,
                closed: true,
            }],
            &color,
        )?;
    }
    marks.finish(rect)
}

fn append(commands: &mut Vec<PathOperator>, quad: &Quad, style: Style, count: usize) -> Result<()> {
    let (bottom, top) = match style {
        Style::Highlight => (0.0, 1.0),
        Style::Underline => (0.0, 1.0 / 12.0),
        Style::StrikeOut => (11.0 / 24.0, 13.0 / 24.0),
        Style::Squiggly => return wave(commands, quad, count),
    };
    for (index, (u, v)) in [(0.0, bottom), (1.0, bottom), (1.0, top), (0.0, top)]
        .into_iter()
        .enumerate()
    {
        let point = quad.at(u, v)?;
        let values = [point.0, point.1];
        commands.push(if index == 0 {
            PathOperator::Move(values)
        } else {
            PathOperator::Line(values)
        });
    }
    Ok(())
}

fn wave(commands: &mut Vec<PathOperator>, quad: &Quad, count: usize) -> Result<()> {
    for index in 0..=count {
        let v = if index.is_multiple_of(2) {
            0.0
        } else {
            1.0 / 12.0
        };
        let point = quad.at(index as f64 / count as f64, v)?;
        let values = [point.0, point.1];
        commands.push(if index == 0 {
            PathOperator::Move(values)
        } else {
            PathOperator::Line(values)
        });
    }
    for index in (0..=count).rev() {
        let v = if index.is_multiple_of(2) {
            1.0 / 24.0
        } else {
            1.0 / 8.0
        };
        let point = quad.at(index as f64 / count as f64, v)?;
        commands.push(PathOperator::Line([point.0, point.1]));
    }
    Ok(())
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite positive step count bounded by at most half of usize::MAX before conversion"
)]
fn steps(quad: &Quad, limits: &pagura_core::Limits) -> Result<usize> {
    let (width, height) = quad.extent()?;
    if width == 0.0 || height == 0.0 {
        return Ok(1);
    }
    let count = (width / (height / 6.0)).ceil().max(1.0);
    if !count.is_finite() || count > (limits.path_segments / 2).min(limits.operations / 2) as f64 {
        return Err(Error::limit("squiggly segment budget exceeded"));
    }
    Ok(count as usize)
}
