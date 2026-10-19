//! Inverse wavelet synthesis, T.800 F.2–F.3.

use pagura_core::Result;

use super::io::{Budget, area, usize_from};

pub(super) struct Plane {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    // An empty buffer denotes an identically zero plane, including nonempty grids.
    pub values: Vec<f64>,
    // Conservative support: rows outside this interval are exactly zero.
    pub rows: std::ops::Range<usize>,
}

impl Plane {
    pub fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self {
            x0,
            y0,
            width: x1 - x0,
            height: y1 - y0,
            values: Vec::new(),
            rows: 0..0,
        }
    }

    pub fn include_rows(&mut self, rows: std::ops::Range<usize>) {
        if !rows.is_empty() {
            self.rows = if self.rows.is_empty() {
                rows
            } else {
                self.rows.start.min(rows.start)..self.rows.end.max(rows.end)
            };
        }
    }

    pub fn materialize(&mut self, budget: &mut Budget<'_>) -> Result<()> {
        if self.values.is_empty() {
            self.values = budget.filled(area(self.width, self.height)?, 0.0)?;
        }
        Ok(())
    }
}

const K: f64 = 1.230_174_104_914_001;

// Horizontal scaling is fused into interleaving; vertical scaling follows row synthesis.
fn inverse_line(line: &mut [f64], origin: u32, reversible: bool) -> Result<()> {
    if line.len() == 1 {
        if origin % 2 == 1 {
            line[0] *= 0.5;
        }
        return Ok(());
    }
    let low = usize::from(!origin.is_multiple_of(2));
    let high = 1 - low;
    if reversible {
        lift(line, low, |sum| -((sum + 2.0) / 4.0).floor());
        lift(line, high, |sum| (sum / 2.0).floor());
    } else {
        lift(line, low, |sum| -0.443_506_852_043_971 * sum);
        lift(line, high, |sum| -0.882_911_075_530_934 * sum);
        lift(line, low, |sum| 0.052_980_118_572_961 * sum);
        lift(line, high, |sum| 1.586_134_342_059_924 * sum);
    }
    // Integers below this conservative threshold retain exact lifting and RCT arithmetic.
    if reversible && line.iter().any(|v| v.abs() > 281_474_976_710_656.0) {
        return Err(super::unsupported(
            "JPEG 2000 reversible intermediate exceeds exact integer precision",
            0,
        ));
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
    budget: &mut Budget<'_>,
) -> Result<Plane> {
    let [x0, y0, x1, y1] = bounds;
    let bands = [low, high[0], high[1], high[2]];
    let mut result = Plane::new(x0, y0, x1, y1);
    // F.3's lifting and scaling map an all-zero input to zero. Uncoded bands
    // need neither coefficient storage nor synthesis, regardless of grid size.
    if bands.iter().all(|band| band.values.is_empty()) {
        return Ok(result);
    }
    result.materialize(budget)?;
    let width = usize_from(result.width)?;
    let height = usize_from(result.height)?;
    for (orientation, band) in bands.iter().enumerate() {
        if band.values.is_empty() {
            continue;
        }
        let start_x = usize::from((x0 % 2 == 1) ^ (orientation % 2 == 1));
        let start_y = usize::from((y0 % 2 == 1) ^ (orientation >= 2));
        let band_width = usize_from(band.width)?;
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
        // F.3: horizontal lifting cannot spread support between rows. Its two
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
                let scale = if (start_y + y) % 2 == (y0 % 2) as usize {
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
    // F.3.2 requires horizontal synthesis before vertical synthesis. Lift whole
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
        let high = 1 - low;
        if reversible {
            lift_rows(&mut result, low, |sum| -((sum + 2.0) / 4.0).floor(), budget)?;
            lift_rows(&mut result, high, |sum| (sum / 2.0).floor(), budget)?;
        } else {
            lift_rows(&mut result, low, |sum| -0.443_506_852_043_971 * sum, budget)?;
            lift_rows(
                &mut result,
                high,
                |sum| -0.882_911_075_530_934 * sum,
                budget,
            )?;
            lift_rows(&mut result, low, |sum| 0.052_980_118_572_961 * sum, budget)?;
            lift_rows(&mut result, high, |sum| 1.586_134_342_059_924 * sum, budget)?;
        }
        if reversible {
            budget.work(width * result.rows.len())?;
        }
        if reversible
            && result.values[result.rows.start * width..result.rows.end * width]
                .iter()
                .any(|v| v.abs() > 281_474_976_710_656.0)
        {
            return Err(super::unsupported(
                "JPEG 2000 reversible intermediate exceeds exact integer precision",
                0,
            ));
        }
    }
    Ok(result)
}

fn lift_rows(
    plane: &mut Plane,
    parity: usize,
    update: impl Fn(f64) -> f64,
    budget: &mut Budget<'_>,
) -> Result<()> {
    let width = usize_from(plane.width)?;
    let height = usize_from(plane.height)?;
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
                            let mut budget = Budget::new(&limits);
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
                                    super::super::progression::band_bounds(bounds, band)
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
            operations: 0,
            decoded_bytes: 0,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        let low = Plane::new(1, 2, 4, 6);
        let hl = Plane::new(0, 2, 4, 6);
        let lh = Plane::new(1, 1, 4, 6);
        let hh = Plane::new(0, 1, 4, 6);
        for reversible in [false, true] {
            // F.3: zero coefficients reconstruct zero; no image samples are lost.
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
