//! Curve-aware ordering. Endpoint order alone cannot exclude two crossings.
//!
//! Eliminating one quadratic parameter gives a polynomial of degree at most
//! four. Its derivative roots partition the parameter range into monotone
//! intervals; bounded bisection finds all sign-changing roots. Tangencies do
//! not change the winding order but are retained as integration events.

use super::{Piece, Quadratic, cross, integrate, lerp, sub};
use crate::coverage::{Row, Windings, bytes, cached, repair};
use pagura_core::{Error, Result};

/// Reused strip scratch: crossing events and pieces keyed by their x.
#[derive(Debug, Default)]
pub(super) struct Strip {
    events: Vec<f64>,
    keyed: Vec<(Piece, f64, f64)>,
}

impl Strip {
    pub(super) fn retained_bytes(&self) -> usize {
        bytes(&self.events).saturating_add(bytes(&self.keyed))
    }
}

// Integrates one strip of active pieces, splitting it at every crossing.
// Each piece's x is evaluated once per strip boundary and interval midpoint.
pub(super) fn strip(
    pieces: &mut [Piece],
    lo: f64,
    hi: f64,
    winding: &mut Windings,
    row: &mut Row,
    scratch: &mut Strip,
) -> Result<()> {
    let Strip { events, keyed } = scratch;
    events.clear();
    row.charge(pieces.len())?;
    keyed.clear();
    keyed.extend(pieces.iter().map(|&p| (p, p.x(lo), p.x(hi))));
    repair(
        keyed,
        |a, b| cached(a.0.edge, a.1, b.0.edge, b.1, false),
        row,
    )?;
    // Since every piece is x-monotone, disjoint x extents are a complete
    // no-crossing certificate, including curves with two intersections.
    let disjoint = keyed
        .windows(2)
        .all(|pair| pair[0].1.max(pair[0].2) <= pair[1].1.min(pair[1].2));
    if !disjoint {
        for (i, &(a, ax, bx)) in keyed.iter().enumerate() {
            for &(b, cx, dx) in &keyed[i + 1..] {
                row.charge(1)?;
                if ax.max(bx) < cx.min(dx) || cx.max(dx) < ax.min(bx) {
                    continue;
                }
                let mut roots = Vec::with_capacity(4);
                intersections(a, b, lo, hi, &mut roots, row)?;
                for y in roots {
                    if y > lo && y < hi {
                        if events.len() >= row.maximum_events {
                            return Err(Error::limit("quadratic crossing event budget exceeded"));
                        }
                        events.push(y);
                    }
                }
            }
        }
    }
    row.sorted(events.len())?;
    events.sort_unstable_by(f64::total_cmp);
    events.dedup();
    let mut start = lo;
    for &end in events.iter().chain(std::iter::once(&hi)) {
        if end > start {
            let mid = start.midpoint(end);
            for (piece, x, _) in keyed.iter_mut() {
                *x = piece.x(mid);
            }
            repair(
                keyed,
                |a, b| cached(a.0.edge, a.1, b.0.edge, b.1, false),
                row,
            )?;
            winding.reset();
            for &(piece, _, _) in keyed.iter() {
                let sign = winding.step(piece.edge)?;
                if let Some(curve) = piece.quadratic() {
                    integrate(row, curve, start, end, sign)?;
                } else {
                    row.edge(piece.edge, start, end, sign)?;
                }
            }
        }
        start = end;
    }
    // The last interval's order seeds the next strip's repair.
    for (piece, &(ordered, _, _)) in pieces.iter_mut().zip(keyed.iter()) {
        *piece = ordered;
    }
    Ok(())
}

fn as_curve(piece: Piece) -> Quadratic {
    piece.quadratic().unwrap_or(Quadratic {
        points: [
            piece.edge.a,
            lerp(piece.edge.a, piece.edge.b, 0.5),
            piece.edge.b,
        ],
    })
}

fn intersections(
    a: Piece,
    b: Piece,
    lo: f64,
    hi: f64,
    out: &mut Vec<f64>,
    row: &mut Row,
) -> Result<()> {
    let (mut a, mut b) = (as_curve(a), as_curve(b));
    let determinant = |curve: Quadratic| {
        let [_, v, c] = curve.coefficients();
        cross(c, v).abs()
    };
    if determinant(a) > determinant(b) {
        std::mem::swap(&mut a, &mut b);
    }
    let [p, v, c] = a.coefficients();
    let [q, w, d] = b.coefficients();
    let r = [sub(p, q), v, c];
    let det = cross(d, w);
    let mut coefficients = [0.0; 5];
    if det == 0.0 {
        let direction = if w.x == 0.0 && w.y == 0.0 { d } else { w };
        for (coefficient, r) in coefficients.iter_mut().zip(r) {
            *coefficient = cross(direction, r);
        }
    } else {
        let linear = r.map(|r| cross(d, r));
        for i in 0..3 {
            coefficients[i] += det * cross(w, r[i]);
            for j in 0..3 {
                coefficients[i + j] += linear[i] * linear[j];
            }
        }
    }
    let magnitude = coefficients.iter().map(|c| c.abs()).fold(0.0, f64::max);
    if magnitude == 0.0 {
        // Coincident supporting curves have no isolated crossing. Their
        // winding transitions at the same x still combine in the usual way.
        return Ok(());
    }
    for value in &mut coefficients {
        *value /= magnitude;
    }
    let start = a.parameter(lo, false);
    let end = a.parameter(hi, false);
    let mut roots = Vec::with_capacity(4);
    polynomial_roots(&coefficients, start, end, &mut roots, row)?;
    for t in roots {
        let point = a.at(t);
        let other = b.at_y(point.y);
        // The resultant describes the full supporting parabola, not only the
        // selected monotone arc. Reject the other branch and clipped roots.
        // A near-double root can be thousands of ulps off. The tolerance stays
        // loose: an extra event only splits the strip, a missed one misorders it.
        let tolerance = 1e-6 * point.x.abs().max(other.x.abs()).max(1.0);
        if point.y >= lo && point.y <= hi && (point.x - other.x).abs() <= tolerance {
            out.push(point.y);
        }
    }
    Ok(())
}

fn evaluate(coefficients: &[f64], t: f64) -> f64 {
    coefficients
        .iter()
        .rev()
        .fold(0.0, |value, c| value * t + c)
}

fn polynomial_roots(
    coefficients: &[f64],
    lo: f64,
    hi: f64,
    out: &mut Vec<f64>,
    row: &mut Row,
) -> Result<()> {
    let degree = coefficients.iter().rposition(|c| *c != 0.0).unwrap_or(0);
    if degree == 0 {
        return Ok(());
    }
    if degree == 1 {
        let root = -coefficients[0] / coefficients[1];
        if root >= lo && root <= hi {
            out.push(root);
        }
        return Ok(());
    }
    let mut derivative = [0.0; 4];
    for i in 1..=degree {
        derivative[i - 1] = i as f64 * coefficients[i];
    }
    let mut stationary = Vec::with_capacity(degree + 1);
    polynomial_roots(&derivative[..degree], lo, hi, &mut stationary, row)?;
    stationary.sort_unstable_by(f64::total_cmp);
    stationary.push(hi);
    let mut start = lo;
    for end in stationary {
        let (mut left, mut right) = (start, end);
        let mut value = evaluate(coefficients, left);
        let last = evaluate(coefficients, right);
        if value == 0.0 {
            out.push(left);
        }
        if (value < 0.0 && last > 0.0) || (value > 0.0 && last < 0.0) {
            // 64 iterations exceed f64's mantissa; the midpoint stop avoids
            // iterations that cannot refine the bracket further.
            for _ in 0..64 {
                row.charge(1)?;
                let mid = left.midpoint(right);
                if mid == left || mid == right {
                    break;
                }
                let next = evaluate(coefficients, mid);
                if next == 0.0 {
                    left = mid;
                    right = mid;
                    break;
                }
                if next.is_sign_negative() == value.is_sign_negative() {
                    left = mid;
                    value = next;
                } else {
                    right = mid;
                }
            }
            out.push(left.midpoint(right));
        }
        if last.abs() <= 32.0 * f64::EPSILON {
            out.push(end);
        }
        start = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::Edge;
    use crate::{FillRule, Point};

    #[test]
    fn two_crossings_with_identical_endpoint_order_are_found() {
        let piece = |points: [Point; 3]| Piece {
            edge: Edge {
                a: points[0],
                b: points[2],
                winding: 1,
                group: 0,
                rule: FillRule::NonZero,
                id: 0,
            },
            control: Some(points[1]),
        };
        let a = piece([
            Point::new(0.0, 0.0),
            Point::new(0.5, 0.5),
            Point::new(1.0, 1.0),
        ]);
        let b = piece([
            Point::new(0.1875, 0.0),
            Point::new(0.1875, 0.5),
            Point::new(1.1875, 1.0),
        ]);
        let mut row = Row {
            maximum: 10000,
            ..Row::default()
        };
        let mut roots = Vec::new();
        intersections(a, b, 0.0, 1.0, &mut roots, &mut row).unwrap();
        roots.sort_unstable_by(f64::total_cmp);
        assert_eq!(roots.len(), 2);
        for (y, expected) in roots.into_iter().zip([0.25, 0.75]) {
            assert!((y - expected).abs() < 1e-12);
            assert!((a.x(y) - b.x(y)).abs() < 1e-12);
        }
    }

    #[test]
    fn quartic_isolation_keeps_all_four_simple_roots() {
        let mut row = Row {
            maximum: 10000,
            ..Row::default()
        };
        let mut roots = Vec::new();
        polynomial_roots(
            &[0.0384, -0.4, 1.4, -2.0, 1.0],
            0.0,
            1.0,
            &mut roots,
            &mut row,
        )
        .unwrap();
        roots.sort_unstable_by(f64::total_cmp);
        assert_eq!(roots.len(), 4);
        for (actual, expected) in roots.iter().zip([0.2, 0.4, 0.6, 0.8]) {
            assert!((actual - expected).abs() < 1e-12);
        }
    }
}
