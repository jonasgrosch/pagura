//! Fixed eight-arc rounded rectangles for Table 166 border radii.

use super::{Marks, Subpath};
use pagura_core::{Error, Rect, Result};

impl Marks<'_> {
    /// Two tangent cubics per corner, using the same 45-degree circle
    /// construction as ellipse artwork. Radii independently clamp to half the
    /// centreline bounds; either zero radius gives a sharp corner by reader policy.
    pub(super) fn rounded_rectangle(&mut self, bounds: Rect, radii: [f64; 2]) -> Result<()> {
        use core::f64::consts::FRAC_1_SQRT_2 as Q;
        const K: f64 = 0.265_216_489_839_544;
        const CORNERS: [[(f64, f64); 3]; 4] = [
            [(0.0, -1.0), (Q, -Q), (1.0, 0.0)],
            [(1.0, 0.0), (Q, Q), (0.0, 1.0)],
            [(0.0, 1.0), (-Q, Q), (-1.0, 0.0)],
            [(-1.0, 0.0), (-Q, -Q), (0.0, -1.0)],
        ];
        if !bounds.width().is_finite() || !bounds.height().is_finite() {
            return Err(Error::limit("rounded border extent exceeds numeric range"));
        }
        let rx = radii[0].min(bounds.width() / 2.0);
        let ry = radii[1].min(bounds.height() / 2.0);
        if rx == 0.0 || ry == 0.0 {
            return self.path(&[Subpath::Rect {
                x: bounds.x0(),
                y: bounds.y0(),
                width: bounds.width(),
                height: bounds.height(),
            }]);
        }
        let centers = [
            (bounds.x1() - rx, bounds.y0() + ry),
            (bounds.x1() - rx, bounds.y1() - ry),
            (bounds.x0() + rx, bounds.y1() - ry),
            (bounds.x0() + rx, bounds.y0() + ry),
        ];
        for (index, ((cx, cy), points)) in centers.into_iter().zip(CORNERS).enumerate() {
            let [(x, y), _, _] = points;
            self.numbers(&[cx + rx * x, cy + ry * y])?;
            self.content.text(if index == 0 { "m\n" } else { "l\n" })?;
            for ((x0, y0), (x1, y1)) in points.into_iter().zip(points.into_iter().skip(1)) {
                self.numbers(&[
                    cx + rx * (x0 - K * y0),
                    cy + ry * (y0 + K * x0),
                    cx + rx * (x1 + K * y1),
                    cy + ry * (y1 - K * x1),
                    cx + rx * x1,
                    cy + ry * y1,
                ])?;
                self.content.text("c\n")?;
            }
        }
        self.content.text("h\n")
    }
}
