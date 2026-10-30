//! The same bounded border geometry in a fresh filled-widget appearance.

use super::{Border, relief};
use crate::annotation::marks::{Color, Paint, rounded, write_polyline};
use crate::program::Program;
use pagura_core::{Error, Rect, Result};

impl Border {
    /// Writes a form border in the caller's saved graphics state. These fresh
    /// form streams use the initial opaque state and need no alpha resources.
    pub(crate) fn paint_form(&self, out: &mut Program, bbox: Rect, color: &[f64]) -> Result<()> {
        if self.width == 0.0 {
            return Ok(());
        }
        if !bbox.width().is_finite() || !bbox.height().is_finite() {
            return Err(Error::limit("widget border extent exceeds numeric range"));
        }
        let color = Color::from_components(color);
        if let Some(kind) = self.relief {
            if let Some(facets) = relief::facets(bbox, self.width)? {
                for (points, shade) in facets.iter().zip(kind.shades(&color)) {
                    shade.write(out, Paint::Fill)?;
                    write_polyline(out, points, true)?;
                    out.text("f\n")?;
                }
            }
            return Ok(());
        }
        color.write(out, Paint::Stroke)?;
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
        } else if self.radii.contains(&0.0) {
            out.text(&format!(
                "{} {} {} {} re S\n",
                bbox.x0() + inset,
                bbox.y0() + inset,
                bbox.width() - 2.0 * inset,
                bbox.height() - 2.0 * inset
            ))?;
        } else {
            let bounds = Rect::from_corners(
                bbox.x0() + inset,
                bbox.y0() + inset,
                bbox.x1() - inset,
                bbox.y1() - inset,
            )
            .ok_or(Error::limit("widget border bounds exceed numeric range"))?;
            rounded::write(out, bounds, self.radii)?;
            out.text("S\n")?;
        }
        Ok(())
    }
}
