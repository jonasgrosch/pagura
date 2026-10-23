//! Source-fixed error evidence for one stored initial polygon mesh.

use super::AREA;
use crate::Point;
use crate::flatten::Hull;
use pagura_core::{Error, Meter, Result};

/// Accepted leaf hulls from the exact subdivision that made a polygon.
/// Tiny hulls contribute a global residual; the rest are sorted by top,
/// with prefix maxima of their bottoms for bounded, metered queries.
#[derive(Debug)]
pub(super) struct Proof {
    hulls: Vec<Hull>,
    bottoms: Vec<f64>,
    residual: f64,
}

impl Proof {
    /// Takes ownership of already charged hull capacity. Every additional
    /// allocation and traversal is charged before it is performed.
    pub(super) fn new(mut hulls: Vec<Hull>, bytes: &mut Meter, work: &mut Meter) -> Result<Self> {
        let hull_bytes = hulls.capacity().saturating_mul(size_of::<Hull>());
        let before = bytes.used().saturating_sub(hull_bytes);
        work.charge(hulls.len())?;
        let finite = hulls.iter().all(|hull| {
            hull.area.is_finite()
                && hull.area >= 0.0
                && hull.bounds.iter().all(|v| v.is_finite())
                && hull.bounds[0] < hull.bounds[2]
                && hull.bounds[1] < hull.bounds[3]
        });
        if !finite {
            drop(hulls);
            bytes.refund(hull_bytes);
            return Ok(Self {
                hulls: Vec::new(),
                bottoms: Vec::new(),
                residual: f64::INFINITY,
            });
        }

        let cap = AREA * 0.5;
        // The threshold only selects folding candidates. The checked
        // outward sum below, not this rounded division, proves the cap.
        let threshold = u32::try_from(hulls.len())
            .ok()
            .filter(|&n| n != 0)
            .map_or(0.0, |n| cap / f64::from(n));
        let mut residual = 0.0;
        work.charge(hulls.len())?;
        hulls.retain(|hull| {
            let next = (residual + hull.area).next_up();
            if hull.area <= threshold && next <= cap {
                residual = next;
                false
            } else {
                true
            }
        });
        if !residual.is_finite() || residual > cap {
            return Err(Error::limit("glyph hull residual exceeds area allowance"));
        }

        work.charge_product(hulls.len(), hulls.len().max(1).ilog2() as usize + 2)?;
        hulls.sort_unstable_by(|a, b| {
            a.bounds[1]
                .total_cmp(&b.bounds[1])
                .then(a.bounds[3].total_cmp(&b.bounds[3]))
                .then(a.bounds[0].total_cmp(&b.bounds[0]))
                .then(a.bounds[2].total_cmp(&b.bounds[2]))
                .then(a.area.total_cmp(&b.area))
        });
        let mut bottoms = Vec::new();
        bytes.reserve_capacity(&mut bottoms, hulls.len())?;
        work.charge(hulls.len())?;
        let mut bottom = f64::NEG_INFINITY;
        for hull in &hulls {
            bottom = bottom.max(hull.bounds[3]);
            bottoms.push(bottom);
        }
        let proof = Self {
            hulls,
            bottoms,
            residual,
        };
        bytes.refund(
            bytes
                .used()
                .saturating_sub(before.saturating_add(proof.retained_bytes())),
        );
        Ok(proof)
    }

    /// An outward area bound for this initial mesh within a placed target.
    /// The inverse bounds enclose exact subtraction of the stored values;
    /// placement, primitive and clipping allowances remain separate.
    pub(super) fn error(&self, bounds: [f64; 4], origin: Point, work: &mut Meter) -> Result<f64> {
        work.charge(1)?;
        if !bounds.iter().all(|v| v.is_finite()) || !origin.is_finite() {
            return Ok(f64::INFINITY);
        }
        if bounds[0] > bounds[2] || bounds[1] > bounds[3] {
            return Ok(f64::INFINITY);
        }
        if bounds[0] == bounds[2] || bounds[1] == bounds[3] {
            return Ok(0.0);
        }
        if !self.residual.is_finite() {
            return Ok(f64::INFINITY);
        }
        let target = [
            (bounds[0] - origin.x).next_down(),
            (bounds[1] - origin.y).next_down(),
            (bounds[2] - origin.x).next_up(),
            (bounds[3] - origin.y).next_up(),
        ];
        if !target.iter().all(|v| v.is_finite()) {
            return Ok(f64::INFINITY);
        }
        work.charge_product(2, self.hulls.len().max(1).ilog2() as usize + 1)?;
        let end = self
            .hulls
            .partition_point(|hull| hull.bounds[1] < target[3]);
        let first = self.bottoms[..end].partition_point(|&bottom| bottom <= target[1]);
        work.charge(end - first)?;
        let mut error = self.residual;
        for hull in &self.hulls[first..end] {
            if hull.intersects(target) {
                error = (error + hull.area).next_up();
            }
        }
        Ok(error)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.hulls
            .capacity()
            .saturating_mul(size_of::<Hull>())
            .saturating_add(self.bottoms.capacity().saturating_mul(size_of::<f64>()))
    }
}
