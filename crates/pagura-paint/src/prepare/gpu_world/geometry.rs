//! Bounded source-local vertical edge indices, reused at every view scale.
use super::{Builder, PAD, STEP, Source, float, index};
use pagura_core::{Error, Result};
use pagura_raster::Point;
impl Builder {
    pub(super) fn edge(&mut self, points: [Point; 3], curve: bool, origin: [u32; 2]) -> Result<()> {
        self.work.charge(1)?;
        self.bytes.reserve(&mut self.edges, 8)?;
        self.edges.push(u32::from(curve));
        for p in points {
            self.edges.push(float(p.x - f64::from(origin[0]))?);
            self.edges.push(float(p.y - f64::from(origin[1]))?);
        }
        self.edges.push(0);
        Ok(())
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite bounded bucket indices"
    )]
    pub(super) fn source(&mut self, start: usize) -> Result<Source> {
        let end = self.edges.len() / 8;
        let value = |i| f64::from(f32::from_bits(self.edges[i]));
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for edge in start..end {
            for j in [2, 4, 6] {
                let y = value(edge * 8 + j);
                lo = lo.min(y);
                hi = hi.max(y);
            }
        }
        if start == end {
            lo = 0.0;
            hi = 0.0;
        }
        let top = ((lo - PAD) / STEP).floor() * STEP;
        let count = ((hi + PAD - top) / STEP).ceil().max(1.0);
        if !count.is_finite() || count > 1_000_000.0 {
            return Err(Error::limit("world source rows"));
        }
        let count = count as usize;
        let mut buckets = Vec::new();
        self.bytes.reserve(&mut buckets, count)?;
        buckets.resize_with(count, Vec::<u32>::new);
        let mut temporary = count * size_of::<Vec<u32>>();
        for edge in start..end {
            let lo = [2, 4, 6]
                .map(|j| value(edge * 8 + j))
                .into_iter()
                .fold(f64::INFINITY, f64::min);
            let hi = [2, 4, 6]
                .map(|j| value(edge * 8 + j))
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max);
            let a = (((lo - PAD - top) / STEP).floor().max(0.0) as usize).min(count);
            let b = (((hi + PAD - top) / STEP).ceil().max(0.0) as usize).min(count);
            for bucket in &mut buckets[a..b] {
                self.work.charge(1)?;
                self.bytes.reserve(bucket, 1)?;
                bucket.push(index(edge)?);
                temporary += 4;
            }
        }
        let source = Source {
            row: index(self.rows.len() / 2)?,
            count: index(count)?,
            top,
            edges: index(end - start)?,
            id: self.source_count,
        };
        self.source_count += 1;
        for bucket in buckets {
            self.bytes.reserve(&mut self.rows, 2)?;
            let start = index(self.row_refs.len())?;
            self.bytes.reserve(&mut self.row_refs, bucket.len())?;
            self.row_refs.extend(bucket);
            self.rows.extend([start, index(self.row_refs.len())?]);
        }
        self.bytes.refund(temporary);
        Ok(source)
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite row indices clamped to the allocated source rows"
    )]
    pub(super) fn charge_shader(&mut self) -> Result<()> {
        let number = |at| f64::from(f32::from_bits(self.marks[at]));
        for (bin, refs) in self.bins.iter().enumerate() {
            for &id in refs {
                let m = id as usize * 24;
                let y = f64::from(index(bin / self.columns as usize * 8)?)
                    - f64::from(self.marks[m + 9])
                    - number(m + 17);
                let count = f64::from(self.marks[m + 5]);
                let top = number(m + 12);
                let a = (((y - top) / STEP).floor() - 1.0).clamp(0.0, count) as usize;
                let b = (((y + STEP - top) / STEP).ceil() + 1.0).clamp(0.0, count) as usize;
                let mut weight = 0;
                for row in a..b {
                    let at = (self.marks[m + 4] as usize + row) * 2;
                    weight = weight.max(self.rows[at + 1] - self.rows[at]);
                }
                self.work
                    .charge((weight as usize + 1).saturating_mul(256))?;
            }
        }
        Ok(())
    }
}
