//! JPEG 2000 inverse lifting; T.800 primary-text review pending.

use pagura_core::Result;

use super::io::{Budget, area, usize_from};

pub(super) struct Plane {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub values: Vec<f64>,
}

impl Plane {
    pub fn new(x0: u32, y0: u32, x1: u32, y1: u32, budget: &mut Budget<'_>) -> Result<Self> {
        Ok(Self {
            x0,
            y0,
            width: x1 - x0,
            height: y1 - y0,
            values: budget.filled(area(x1 - x0, y1 - y0)?, 0.0)?,
        })
    }
}

fn inverse_line(line: &mut [f64], origin: u32, reversible: bool) -> Result<()> {
    if line.len() == 1 {
        if origin % 2 == 1 && reversible {
            line[0] = (line[0] * 0.5).trunc();
        }
        return Ok(());
    }
    let low = usize::from(!origin.is_multiple_of(2));
    let high = 1 - low;
    if reversible {
        lift(line, low, |sum| -((sum + 2.0) / 4.0).floor());
        lift(line, high, |sum| (sum / 2.0).floor());
    } else {
        const K: f64 = 1.230_174_104_914_001;
        for (i, value) in line.iter_mut().enumerate() {
            *value *= if i % 2 == low { K } else { 2.0 / K };
        }
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
    let mut result = Plane::new(x0, y0, x1, y1, budget)?;
    let width = usize_from(result.width)?;
    let height = usize_from(result.height)?;
    budget.work(
        area(result.width, result.height)?
            .checked_mul(16)
            .ok_or_else(|| pagura_core::Error::limit("JPEG 2000 wavelet work overflow"))?,
    )?;
    for (orientation, band) in [low, high[0], high[1], high[2]].iter().enumerate() {
        let start_x = usize::from((x0 % 2 == 1) ^ (orientation % 2 == 1));
        let start_y = usize::from((y0 % 2 == 1) ^ (orientation >= 2));
        let band_width = usize_from(band.width)?;
        for y in 0..usize_from(band.height)? {
            for x in 0..band_width {
                result.values[(2 * y + start_y) * width + 2 * x + start_x] =
                    band.values[y * band_width + x];
            }
        }
    }
    if width != 0 {
        for row in result.values.chunks_exact_mut(width) {
            inverse_line(row, x0, reversible)?;
        }
    }
    let mut column = budget.filled(height, 0.0)?;
    // Reconstruction order is differential-tested; reversible rounding makes it significant.
    for x in 0..width {
        for (y, value) in column.iter_mut().enumerate() {
            *value = result.values[y * width + x];
        }
        inverse_line(&mut column, y0, reversible)?;
        for (y, &value) in column.iter().enumerate() {
            result.values[y * width + x] = value;
        }
    }
    Ok(result)
}
