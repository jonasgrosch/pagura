//! Inverse wavelet synthesis, T.800 F.2–F.3.

use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};

use crate::budget::Budget;

use super::area;

pub(super) struct Plane {
    pub(super) x0: u32,
    pub(super) y0: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    // An empty buffer denotes an identically zero plane, including nonempty grids.
    pub(super) values: Vec<f64>,
    // Conservative support: rows outside this interval are exactly zero.
    pub(super) rows: std::ops::Range<usize>,
}

impl Plane {
    pub(super) fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self {
            x0,
            y0,
            width: x1 - x0,
            height: y1 - y0,
            values: Vec::new(),
            rows: 0..0,
        }
    }

    pub(super) fn include_rows(&mut self, rows: std::ops::Range<usize>) {
        if !rows.is_empty() {
            self.rows = if self.rows.is_empty() {
                rows
            } else {
                self.rows.start.min(rows.start)..self.rows.end.max(rows.end)
            };
        }
    }

    pub(super) fn materialize(&mut self, budget: &mut Budget) -> Result<()> {
        if self.values.is_empty() {
            self.values = budget.filled(area(self.width, self.height)?, 0.0)?;
        }
        Ok(())
    }

    /// Drops the plane and returns its coefficients to the memory budget.
    pub(super) fn release(self, budget: &mut Budget) {
        let count = self.values.len();
        drop(self);
        budget.release::<f64>(count);
    }
}

// The 9/7 lifting parameters and scaling factor (T.800 Table F.4).
const ALPHA: f64 = -1.586_134_342_059_924;
const BETA: f64 = -0.052_980_118_572_961;
const GAMMA: f64 = 0.882_911_075_530_934;
const DELTA: f64 = 0.443_506_852_043_971;
const K: f64 = 1.230_174_104_914_001;

/// The magnitude up to which f64 lifting and component transform arithmetic
/// on integers is exact: 2^48, conservatively below the 53-bit significand.
const EXACT: f64 = 281_474_976_710_656.0;

/// The samples a lifting step updates: those of the low-pass or those of
/// the high-pass band.
#[derive(Clone, Copy)]
enum Phase {
    Low,
    High,
}

impl Phase {
    /// The index of the first updated sample, given that of the first
    /// low-pass sample.
    const fn first(self, low: usize) -> usize {
        match self {
            Self::Low => low,
            Self::High => 1 - low,
        }
    }
}

/// One lifting step: the samples it updates and the update.
type Step = (Phase, fn(f64) -> f64);

/// The inverse lifting steps in order: each updates its samples by a
/// function of the sum of their two neighbours (T.800 F.3.8.1 for the 5/3
/// filter, T.800 F.3.8.2 for the 9/7 filter).
fn steps(reversible: bool) -> &'static [Step] {
    const REVERSIBLE: [Step; 2] = [
        (Phase::Low, |sum| -((sum + 2.0) / 4.0).floor()),
        (Phase::High, |sum| (sum / 2.0).floor()),
    ];
    const IRREVERSIBLE: [Step; 4] = [
        (Phase::Low, |sum| -DELTA * sum),
        (Phase::High, |sum| -GAMMA * sum),
        (Phase::Low, |sum| -BETA * sum),
        (Phase::High, |sum| -ALPHA * sum),
    ];
    if reversible {
        &REVERSIBLE
    } else {
        &IRREVERSIBLE
    }
}

/// Rejects reversible intermediates beyond exact arithmetic.
fn check_exact(values: &[f64]) -> Result<()> {
    if values.iter().any(|v| v.abs() > EXACT) {
        return Err(Error::unsupported(
            super::UNSUPPORTED,
            "JPEG 2000 reversible intermediate exceeds exact integer precision",
        ));
    }
    Ok(())
}

// Horizontal scaling is fused into interleaving; vertical scaling follows row synthesis.
fn inverse_line(line: &mut [f64], origin: u32, reversible: bool) -> Result<()> {
    if line.len() == 1 {
        // T.800 F.3.6: a lone high-pass sample is halved.
        if origin % 2 == 1 {
            line[0] *= 0.5;
        }
        return Ok(());
    }
    let low = usize::from(!origin.is_multiple_of(2));
    for &(phase, update) in steps(reversible) {
        lift(line, phase.first(low), update);
    }
    if reversible {
        check_exact(line)?;
    }
    Ok(())
}

fn lift(line: &mut [f64], parity: usize, update: impl Fn(f64) -> f64) {
    for index in (parity..line.len()).step_by(2) {
        let left = if index == 0 { 1 } else { index - 1 };
        let right = if index + 1 == line.len() {
            index - 1
        } else {
            index + 1
        };
        line[index] += update(line[left] + line[right]);
    }
}

pub(super) fn synthesize(
    low: &Plane,
    high: [&Plane; 3],
    bounds: [u32; 4],
    reversible: bool,
    budget: &mut Budget,
) -> Result<Plane> {
    let [x0, y0, x1, y1] = bounds;
    let bands = [low, high[0], high[1], high[2]];
    let mut result = Plane::new(x0, y0, x1, y1);
    // T.800 F.3's lifting and scaling map an all-zero input to zero. Uncoded bands
    // need neither coefficient storage nor synthesis, regardless of grid size.
    if bands.iter().all(|band| band.values.is_empty()) {
        return Ok(result);
    }
    result.materialize(budget)?;
    let width = u32_to_usize(result.width);
    let height = u32_to_usize(result.height);
    for (orientation, band) in bands.iter().enumerate() {
        if band.values.is_empty() {
            continue;
        }
        let start_x = usize::from((x0 % 2 == 1) ^ (orientation % 2 == 1));
        let start_y = usize::from((y0 % 2 == 1) ^ (orientation >= 2));
        let band_width = u32_to_usize(band.width);
        let scale = if reversible || width <= 1 {
            1.0
        } else if orientation % 2 == 0 {
            K
        } else {
            1.0 / K
        };
        budget.work(band_width * band.rows.len())?;
        result.include_rows(2 * band.rows.start + start_y..2 * (band.rows.end - 1) + start_y + 1);
        for y in band.rows.clone() {
            for x in 0..band_width {
                result.values[(2 * y + start_y) * width + 2 * x + start_x] =
                    band.values[y * band_width + x] * scale;
            }
        }
    }
    if width != 0 {
        // T.800 F.3: horizontal lifting cannot spread support between rows. Its two
        // full-row visits include the 5/3 range check or the four 9/7 half-lifts.
        let visits = if width > 1 {
            2
        } else {
            usize::from(x0 % 2 == 1)
        };
        budget
            .work(width * result.rows.len() * (visits + usize::from(!reversible && height > 1)))?;
        let start_y = result.rows.start;
        let rows = &mut result.values[result.rows.start * width..result.rows.end * width];
        for (y, row) in rows.chunks_exact_mut(width).enumerate() {
            inverse_line(row, x0, reversible)?;
            if !reversible && height > 1 {
                let scale = if (start_y + y) % 2 == usize::from(y0 % 2 == 1) {
                    K
                } else {
                    1.0 / K
                };
                for value in row {
                    *value *= scale;
                }
            }
        }
    }
    // T.800 F.3.2 requires horizontal synthesis before vertical synthesis. Lift whole
    // rows against their neighbours: columns are independent, so the same
    // arithmetic needs neither column gathers/scatters nor a scratch buffer.
    if height == 1 {
        if y0 % 2 == 1 {
            budget.work(result.values.len())?;
            for value in &mut result.values {
                *value *= 0.5;
            }
        }
    } else {
        let low = usize::from(!y0.is_multiple_of(2));
        for &(phase, update) in steps(reversible) {
            lift_rows(&mut result, phase.first(low), update, budget)?;
        }
        if reversible {
            budget.work(width * result.rows.len())?;
            check_exact(&result.values[result.rows.start * width..result.rows.end * width])?;
        }
    }
    Ok(result)
}

fn lift_rows(
    plane: &mut Plane,
    parity: usize,
    update: impl Fn(f64) -> f64,
    budget: &mut Budget,
) -> Result<()> {
    let width = u32_to_usize(plane.width);
    let height = u32_to_usize(plane.height);
    // A lift can spread nonzero support by at most one row. Keeping an interval
    // is conservative when there are holes, and needs no per-coefficient flags.
    let start = plane.rows.start.saturating_sub(1);
    let start = start + usize::from(start % 2 != parity);
    let end = (plane.rows.end + 1).min(height);
    if start >= end {
        return Ok(());
    }
    let count = (end - start).div_ceil(2);
    budget.work(count * width)?;
    plane.include_rows(start..start + 2 * (count - 1) + 1);
    for y in (start..end).step_by(2) {
        let (before, current) = plane.values.split_at_mut(y * width);
        let (row, after) = current.split_at_mut(width);
        let above = if y == 0 {
            &after[..width]
        } else {
            &before[before.len() - width..]
        };
        let below = if y + 1 == height {
            above
        } else {
            &after[..width]
        };
        for ((value, above), below) in row.iter_mut().zip(above).zip(below) {
            *value += update(above + below);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Budget, Plane, synthesize};
    use pagura_core::Limits;

    #[test]
    fn sparse_rows_match_full_lifting_at_boundaries_and_singletons()
    -> Result<(), Box<dyn std::error::Error>> {
        for (x0, y0) in [(0_u32, 0_u32), (1, 0), (0, 1), (1, 1)] {
            for (width, height) in [(1, 1), (1, 11), (11, 1), (2, 2), (7, 9), (9, 17)] {
                let bounds = [x0, y0, x0 + width, y0 + height];
                for reversible in [false, true] {
                    for orientation in 0..4 {
                        for position in 0..3 {
                            let limits = Limits::default();
                            let mut budget = Budget::new(&limits, crate::jpx::MESSAGES);
                            let mut bands = Vec::new();
                            let mut full = Vec::new();
                            for band in 0..4 {
                                let band_bounds = if band == 0 {
                                    [
                                        x0.div_ceil(2),
                                        y0.div_ceil(2),
                                        bounds[2].div_ceil(2),
                                        bounds[3].div_ceil(2),
                                    ]
                                } else {
                                    crate::jpx::progression::band_bounds(bounds, band)
                                };
                                let [bx0, by0, bx1, by1] = band_bounds;
                                let mut plane = Plane::new(bx0, by0, bx1, by1);
                                if band == orientation && bx1 > bx0 && by1 > by0 {
                                    plane.materialize(&mut budget)?;
                                    let row = usize::try_from((by1 - by0 - 1) * position / 2)?;
                                    let column = usize::try_from((bx1 - bx0 - 1) * position / 2)?;
                                    let stride = usize::try_from(plane.width)?;
                                    plane.values[row * stride + column] = -97.0;
                                    plane.include_rows(row..row + 1);
                                }
                                let mut reference = Plane::new(bx0, by0, bx1, by1);
                                reference.materialize(&mut budget)?;
                                if !plane.values.is_empty() {
                                    reference.values.copy_from_slice(&plane.values);
                                }
                                reference.include_rows(0..usize::try_from(reference.height)?);
                                bands.push(plane);
                                full.push(reference);
                            }
                            let actual = synthesize(
                                &bands[0],
                                [&bands[1], &bands[2], &bands[3]],
                                bounds,
                                reversible,
                                &mut budget,
                            )?;
                            let expected = synthesize(
                                &full[0],
                                [&full[1], &full[2], &full[3]],
                                bounds,
                                reversible,
                                &mut budget,
                            )?;
                            if actual.values.is_empty() {
                                assert!(expected.values.iter().all(|&v| v == 0.0));
                            } else {
                                assert_eq!(
                                    actual.values, expected.values,
                                    "bounds {bounds:?}, reversible {reversible}, band {orientation}, position {position}"
                                );
                            }
                            let stride = usize::try_from(actual.width)?;
                            for (row, values) in actual.values.chunks_exact(stride).enumerate() {
                                if !actual.rows.contains(&row) {
                                    assert!(values.iter().all(|&v| v == 0.0));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn zero_subbands_preserve_odd_grids_without_coefficient_storage() -> pagura_core::Result<()> {
        let limits = Limits {
            decode_work: 0,
            decode_bytes: 0,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits, crate::jpx::MESSAGES);
        let low = Plane::new(1, 2, 4, 6);
        let hl = Plane::new(0, 2, 4, 6);
        let lh = Plane::new(1, 1, 4, 6);
        let hh = Plane::new(0, 1, 4, 6);
        for reversible in [false, true] {
            // T.800 F.3: zero coefficients reconstruct zero; no image samples are lost.
            let result = synthesize(
                &low,
                [&hl, &lh, &hh],
                [1, 3, 8, 12],
                reversible,
                &mut budget,
            )?;
            assert_eq!(
                (result.x0, result.y0, result.width, result.height),
                (1, 3, 7, 9)
            );
            assert_eq!(result.values, [] as [f64; 0]);
        }
        Ok(())
    }
}
