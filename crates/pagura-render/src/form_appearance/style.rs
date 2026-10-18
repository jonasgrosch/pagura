//! Widget background and border data (12.5.6.19, Tables 168 and 192).
use super::{Program, optional, optional_dictionary};
use crate::border::{Border, BorderStyle};
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_document::Document;

pub(super) struct Style {
    pub inset: f64,
    background: Option<Vec<f64>>,
    border: Option<Vec<f64>>,
    width: f64,
    underline: bool,
    dash: Vec<f64>,
}
impl Style {
    pub(super) fn read(document: &Document, widget: &Dictionary) -> Result<Self> {
        let mk = optional_dictionary(document, widget, b"MK")?;
        if let Some(rotation) = mk
            .map(|d| optional(document, d, b"R"))
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
                    crate::features::APPEARANCE_WIDGET_ROTATION,
                    "rotated widget appearance generation is unavailable",
                ));
            }
        }
        let background = mk.map(|d| color(document, d, b"BG")).transpose()?.flatten();
        let border = mk.map(|d| color(document, d, b"BC")).transpose()?.flatten();
        let Border {
            width,
            style,
            rounded,
        } = Border::read(document, widget)?;
        if rounded {
            return Err(Error::unsupported(
                crate::features::APPEARANCE_BORDER_STYLE,
                "rounded widget borders are unavailable",
            ));
        }
        let (dash, underline) = match style {
            BorderStyle::Solid => (Vec::new(), false),
            BorderStyle::Dashed(dash) => (dash, false),
            BorderStyle::Underline => (Vec::new(), true),
            BorderStyle::Beveled | BorderStyle::Inset | BorderStyle::Other => {
                return Err(Error::unsupported(
                    crate::features::APPEARANCE_BORDER_STYLE,
                    "widget border style is unavailable",
                ));
            }
        };
        Ok(Self {
            inset: if border.is_some() { width + 2.0 } else { 2.0 },
            background,
            border,
            width,
            underline,
            dash,
        })
    }
    pub(super) fn paint(&self, out: &mut Program, bbox: Rect) -> Result<()> {
        out.text("q\n")?;
        if let Some(color) = &self.background {
            paint_color(out, color, false)?;
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
            paint_color(out, color, true)?;
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
fn color(document: &Document, d: &Dictionary, key: &[u8]) -> Result<Option<Vec<f64>>> {
    let Some(value) = optional(document, d, key)? else {
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
fn paint_color(out: &mut Program, color: &[f64], stroke: bool) -> Result<()> {
    for value in color {
        out.text(&format!("{value} "))?;
    }
    out.text(match (color.len(), stroke) {
        (1, false) => "g\n",
        (1, true) => "G\n",
        (3, false) => "rg\n",
        (3, true) => "RG\n",
        (4, false) => "k\n",
        (4, true) => "K\n",
        _ => return Err(Error::syntax("invalid widget colour component count")),
    })
}
fn numbers(document: &Document, value: &Value) -> Result<Vec<f64>> {
    crate::numbers::list(document, value, f64::MAX)
}
