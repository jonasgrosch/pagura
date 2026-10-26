//! Widget background and border data (12.5.6.19, Tables 168 and 192).

use crate::annotation::border::Border;
use crate::features;
use crate::program::Program;
use crate::values::{numbers, optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Rect, Result};
use pagura_document::Document;

/// A widget's MK colours and border, and the text padding they imply.
pub(super) struct Style {
    pub(super) inset: f64,
    background: Option<Vec<f64>>,
    border: Option<Vec<f64>>,
    outline: Border,
}

impl Style {
    pub(super) fn read(document: &Document, widget: &Dictionary) -> Result<Self> {
        let mk = optional_dictionary(document, widget, b"MK")?;
        if let Some(rotation) = mk
            .map(|mk| optional(document, mk, b"R"))
            .transpose()?
            .flatten()
        {
            let rotation = document
                .resolve(rotation)?
                .as_integer()
                .ok_or(Error::syntax("MK R must be an integer"))?;
            if rotation % 90 != 0 {
                return Err(Error::syntax("MK R must be a multiple of 90"));
            }
            if rotation.rem_euclid(360) != 0 {
                return Err(Error::unsupported(
                    features::APPEARANCE_WIDGET_ROTATION,
                    "rotated widget appearance generation is unavailable",
                ));
            }
        }
        let background = mk
            .map(|mk| color(document, mk, b"BG"))
            .transpose()?
            .flatten();
        let border = mk
            .map(|mk| color(document, mk, b"BC"))
            .transpose()?
            .flatten();
        let style = Border::read_rectangular(document, widget)?;
        Ok(Self {
            inset: if border.is_some() {
                style.width + 2.0
            } else {
                2.0
            },
            background,
            border,
            outline: style,
        })
    }

    /// Fills the background and strokes the border inside `bbox`, in a
    /// saved graphics state.
    pub(super) fn paint(&self, out: &mut Program, bbox: Rect) -> Result<()> {
        out.text("q\n")?;
        if let Some(color) = &self.background {
            paint_color(out, color)?;
            out.text(&format!(
                "{} {} {} {} re f\n",
                bbox.x0(),
                bbox.y0(),
                bbox.width(),
                bbox.height()
            ))?;
        }
        if let Some(color) = &self.border {
            self.outline.paint_form(out, bbox, color)?;
        }
        out.text("Q\n")
    }
}

fn color(document: &Document, mk: &Dictionary, key: &[u8]) -> Result<Option<Vec<f64>>> {
    let Some(value) = optional(document, mk, key)? else {
        return Ok(None);
    };
    let components = numbers(document, value)?;
    if !matches!(components.len(), 0 | 1 | 3 | 4)
        || components.iter().any(|v| !(0.0..=1.0).contains(v))
    {
        return Err(Error::syntax("invalid widget appearance colour"));
    }
    Ok((!components.is_empty()).then_some(components))
}

fn paint_color(out: &mut Program, color: &[f64]) -> Result<()> {
    for value in color {
        out.text(&format!("{value} "))?;
    }
    out.text(match color.len() {
        1 => "g\n",
        3 => "rg\n",
        4 => "k\n",
        _ => return Err(Error::syntax("invalid widget colour component count")),
    })
}
