//! Fixed eight-arc rounded rectangles for Table 166 border radii.

use super::write_numbers;
use crate::program::Program;
use pagura_core::{Error, Rect, Result};

/// Two tangent cubics per corner, using the same 45-degree circle
/// construction as ellipse artwork. Radii independently clamp to half the
/// centreline bounds; either zero radius gives a sharp corner by reader policy.
pub(in crate::annotation) fn write(out: &mut Program, bounds: Rect, radii: [f64; 2]) -> Result<()> {
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
        write_numbers(
            out,
            &[bounds.x0(), bounds.y0(), bounds.width(), bounds.height()],
        )?;
        return out.text("re\n");
    }
    let centers = [
        (bounds.x1() - rx, bounds.y0() + ry),
        (bounds.x1() - rx, bounds.y1() - ry),
        (bounds.x0() + rx, bounds.y1() - ry),
        (bounds.x0() + rx, bounds.y0() + ry),
    ];
    for (index, ((cx, cy), points)) in centers.into_iter().zip(CORNERS).enumerate() {
        let [(x, y), _, _] = points;
        write_numbers(out, &[cx + rx * x, cy + ry * y])?;
        out.text(if index == 0 { "m\n" } else { "l\n" })?;
        for ((x0, y0), (x1, y1)) in points.into_iter().zip(points.into_iter().skip(1)) {
            write_numbers(
                out,
                &[
                    cx + rx * (x0 - K * y0),
                    cy + ry * (y0 + K * x0),
                    cx + rx * (x1 + K * y1),
                    cy + ry * (y1 - K * x1),
                    cx + rx * x1,
                    cy + ry * y1,
                ],
            )?;
            out.text("c\n")?;
        }
    }
    out.text("h\n")
}
