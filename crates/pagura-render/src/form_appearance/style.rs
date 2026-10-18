//! Widget background and border data (12.5.6.19, Tables 168 and 192).
use super::{Program, optional, optional_dictionary};
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
    pub fn read(document: &Document, widget: &Dictionary) -> Result<Self> {
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
        let mut width = 1.0;
        let mut underline = false;
        let mut dash = Vec::new();
        if let Some(bs) = optional_dictionary(document, widget, b"BS")? {
            if let Some(value) = optional(document, bs, b"W")? {
                width = number(document, value)?;
            }
            let style = optional(document, bs, b"S")?;
            let style = style
                .map(|v| v.as_name().ok_or(Error::syntax("BS S must be a name")))
                .transpose()?;
            match style.map_or(b"S".as_slice(), pagura_core::Name::as_bytes) {
                b"S" => (),
                b"U" => underline = true,
                b"D" => {
                    dash = if let Some(value) = optional(document, bs, b"D")? {
                        numbers(document, value)?
                    } else {
                        vec![3.0]
                    };
                    if dash.iter().any(|v| *v < 0.0)
                        || !dash.is_empty() && dash.iter().all(|v| *v == 0.0)
                    {
                        return Err(Error::syntax("invalid widget dash pattern"));
                    }
                }
                _ => {
                    return Err(Error::unsupported(
                        crate::features::APPEARANCE_BORDER_STYLE,
                        "widget border style is unavailable",
                    ));
                }
            }
        } else if let Some(value) = optional(document, widget, b"Border")? {
            let border = document
                .resolve(value)?
                .as_array()
                .ok_or(Error::syntax("widget Border must be an array"))?;
            if !matches!(border.len(), 3 | 4) {
                return Err(Error::syntax(
                    "widget Border requires three or four entries",
                ));
            }
            if number(document, &border[0])? != 0.0 || number(document, &border[1])? != 0.0 {
                return Err(Error::unsupported(
                    crate::features::APPEARANCE_BORDER_STYLE,
                    "rounded widget borders are unavailable",
                ));
            }
            width = number(document, &border[2])?;
            if let Some(value) = border.get(3) {
                dash = numbers(document, value)?;
            }
            if dash.iter().any(|v| *v < 0.0) || !dash.is_empty() && dash.iter().all(|v| *v == 0.0) {
                return Err(Error::syntax("invalid widget dash pattern"));
            }
        }
        if width < 0.0 {
            return Err(Error::syntax("negative widget border width"));
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
    pub fn paint(&self, out: &mut Program, bbox: Rect) -> Result<()> {
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
fn number(document: &Document, value: &Value) -> Result<f64> {
    crate::numbers::number(document, value, f64::MAX)
}
