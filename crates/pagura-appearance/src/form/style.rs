//! Widget background and border data (12.5.6.19, Tables 168 and 192).

use crate::features;
use crate::program::Program;
use crate::values::{number, numbers, optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Name, Rect, Result};
use pagura_document::Document;
use pagura_document::source::ReadResult;

/// A widget's MK colours and border, and the text padding they imply.
pub(super) struct Style {
    pub(super) inset: f64,
    background: Option<Vec<f64>>,
    border: Option<Vec<f64>>,
    width: f64,
    underline: bool,
    dash: Vec<f64>,
}

/// A border's width, dash pattern and whether it is an underline.
struct Border {
    width: f64,
    underline: bool,
    dash: Vec<f64>,
}

impl Style {
    pub(super) fn read(document: &Document, widget: &Dictionary) -> ReadResult<Self> {
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
                return Err(Error::syntax("MK R must be a multiple of 90").into());
            }
            if rotation.rem_euclid(360) != 0 {
                return Err(Error::unsupported(
                    features::APPEARANCE_WIDGET_ROTATION,
                    "rotated widget appearance generation is unavailable",
                )
                .into());
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
        let Border {
            width,
            underline,
            dash,
        } = border_style(document, widget)?;
        if width < 0.0 {
            return Err(Error::syntax("negative widget border width").into());
        }
        Ok(Self {
            inset: if border.is_some() { width + 2.0 } else { 2.0 },
            background,
            border,
            width,
            underline,
            dash,
        })
    }

    /// Fills the background and strokes the border inside `bbox`, in a
    /// saved graphics state.
    pub(super) fn paint(&self, out: &mut Program, bbox: Rect) -> Result<()> {
        out.text("q\n")?;
        if let Some(color) = &self.background {
            paint_color(out, color, Operation::Fill)?;
            out.text(&format!(
                "{} {} {} {} re f\n",
                bbox.x0(),
                bbox.y0(),
                bbox.width(),
                bbox.height()
            ))?;
        }
        if let Some(color) = &self.border
            && self.width > 0.0
        {
            paint_color(out, color, Operation::Stroke)?;
            let inset = (self.width / 2.0)
                .min(bbox.width() / 2.0)
                .min(bbox.height() / 2.0);
            out.text(&format!("{} w\n[", self.width))?;
            for value in &self.dash {
                out.text(&format!("{value} "))?;
            }
            out.text("] 0 d\n")?;
            if self.underline {
                out.text(&format!(
                    "{} {} m {} {} l S\n",
                    bbox.x0() + inset,
                    bbox.y0() + inset,
                    bbox.x1() - inset,
                    bbox.y0() + inset
                ))?;
            } else {
                out.text(&format!(
                    "{} {} {} {} re S\n",
                    bbox.x0() + inset,
                    bbox.y0() + inset,
                    bbox.width() - 2.0 * inset,
                    bbox.height() - 2.0 * inset
                ))?;
            }
        }
        out.text("Q\n")
    }
}

/// The border of BS, else of the Border array, else a solid 1-unit border.
fn border_style(document: &Document, widget: &Dictionary) -> ReadResult<Border> {
    let mut border = Border {
        width: 1.0,
        underline: false,
        dash: Vec::new(),
    };
    if let Some(bs) = optional_dictionary(document, widget, b"BS")? {
        if let Some(value) = optional(document, bs, b"W")? {
            border.width = number(document, value)?;
        }
        let style = optional(document, bs, b"S")?
            .map(|value| value.as_name().ok_or(Error::syntax("BS S must be a name")))
            .transpose()?;
        match style.map_or(b"S".as_slice(), Name::as_bytes) {
            b"S" => (),
            b"U" => border.underline = true,
            b"D" => {
                border.dash = match optional(document, bs, b"D")? {
                    Some(value) => numbers(document, value)?,
                    None => vec![3.0],
                };
                validate_dash(&border.dash)?;
            }
            _ => {
                return Err(Error::unsupported(
                    features::APPEARANCE_BORDER_STYLE,
                    "widget border style is unavailable",
                )
                .into());
            }
        }
    } else if let Some(value) = optional(document, widget, b"Border")? {
        let array = document
            .resolve(value)?
            .as_array()
            .ok_or(Error::syntax("widget Border must be an array"))?;
        let [horizontal, vertical, width, rest @ ..] = array else {
            return Err(Error::syntax("widget Border requires three or four entries").into());
        };
        if rest.len() > 1 {
            return Err(Error::syntax("widget Border requires three or four entries").into());
        }
        if number(document, horizontal)? != 0.0 || number(document, vertical)? != 0.0 {
            return Err(Error::unsupported(
                features::APPEARANCE_BORDER_STYLE,
                "rounded widget borders are unavailable",
            )
            .into());
        }
        border.width = number(document, width)?;
        if let Some(value) = rest.first() {
            border.dash = numbers(document, value)?;
        }
        validate_dash(&border.dash)?;
    }
    Ok(border)
}

fn validate_dash(dash: &[f64]) -> Result<()> {
    if dash.iter().any(|v| *v < 0.0) || !dash.is_empty() && dash.iter().all(|v| *v == 0.0) {
        return Err(Error::syntax("invalid widget dash pattern"));
    }
    Ok(())
}

fn color(document: &Document, mk: &Dictionary, key: &[u8]) -> ReadResult<Option<Vec<f64>>> {
    let Some(value) = optional(document, mk, key)? else {
        return Ok(None);
    };
    let components = numbers(document, value)?;
    if !matches!(components.len(), 0 | 1 | 3 | 4)
        || components.iter().any(|v| !(0.0..=1.0).contains(v))
    {
        return Err(Error::syntax("invalid widget appearance colour").into());
    }
    Ok((!components.is_empty()).then_some(components))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Fill,
    Stroke,
}

fn paint_color(out: &mut Program, color: &[f64], operation: Operation) -> Result<()> {
    for value in color {
        out.text(&format!("{value} "))?;
    }
    out.text(match (color.len(), operation) {
        (1, Operation::Fill) => "g\n",
        (1, Operation::Stroke) => "G\n",
        (3, Operation::Fill) => "rg\n",
        (3, Operation::Stroke) => "RG\n",
        (4, Operation::Fill) => "k\n",
        (4, Operation::Stroke) => "K\n",
        _ => return Err(Error::syntax("invalid widget colour component count")),
    })
}
