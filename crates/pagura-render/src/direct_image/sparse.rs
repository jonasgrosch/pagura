//! Sparse rows of a deferred JPEG: only blocks that differ from the white
//! background are converted and filtered.
//!
//! Each converted sample is held as its deficit from white, 255 minus each
//! RGB byte. Background blocks contribute zero, so footprints that see only
//! background need no work.
use super::footprint::{Band, Tap};
use pagura_core::{Error, Limits, Result};
use pagura_image::deferred::{Constant, Planar, deficits, uniform, ycbcr8};
use std::ops::Range;
const NONE: usize = usize::MAX;
pub(super) struct Plan {
    rgb: Vec<[u16; 4]>,
    block_targets: Vec<[usize; 2]>,
    source_runs: Vec<Range<usize>>,
    targets: Vec<Range<usize>>,
    key: Option<[usize; 3]>,
}
impl Plan {
    pub(super) fn new(
        width: usize,
        bands: &[Band],
        taps: &[Tap],
        limits: Limits,
    ) -> Result<Option<Self>> {
        if width > usize::from(u16::MAX) {
            return Ok(None);
        }
        let blocks = width.div_ceil(8);
        let bytes = width
            .checked_mul(size_of::<[u16; 4]>())
            .and_then(|n| {
                blocks
                    .checked_mul(3 * size_of::<[usize; 2]>())
                    .and_then(|b| n.checked_add(b))
            })
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("sparse area plan exceeded"))?;
        let _ = bytes;
        let mut rgb = Vec::new();
        rgb.try_reserve_exact(width)
            .map_err(|_| Error::limit("sparse RGB allocation failed"))?;
        rgb.resize(width, [0; 4]);
        let mut block_targets = Vec::new();
        block_targets
            .try_reserve_exact(blocks)
            .map_err(|_| Error::limit("sparse map allocation failed"))?;
        block_targets.resize(blocks, [NONE, 0]);
        for (i, band) in bands.iter().enumerate() {
            let t = &taps[band.start..band.end];
            for target in &mut block_targets[t[0].sample / 8..=t[t.len() - 1].sample / 8] {
                target[0] = target[0].min(i);
                target[1] = target[1].max(i + 1);
            }
        }
        let mut source_runs = Vec::new();
        source_runs
            .try_reserve_exact(blocks)
            .map_err(|_| Error::limit("sparse run allocation failed"))?;
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(blocks)
            .map_err(|_| Error::limit("sparse target allocation failed"))?;
        Ok(Some(Self {
            rgb,
            block_targets,
            source_runs,
            targets,
            key: None,
        }))
    }
    fn prepare(&mut self, p: &Planar, y: usize) -> bool {
        let key = std::array::from_fn(|i| y * p.geometry(i)[1] / p.maximum()[1] / 8);
        if self.key == Some(key) {
            return true;
        }
        for run in &self.source_runs {
            self.rgb[run.clone()].fill([0; 4]);
        }
        self.key = Some(key);
        self.source_runs.clear();
        self.targets.clear();
        let mut open = None;
        for block in 0..self.block_targets.len() {
            let x = block * 8;
            let end = (x + 8).min(p.width());
            let dirty = (0..3).any(|i| {
                let h = p.geometry(i)[0];
                let first = x * h / p.maximum()[0] / 8;
                let last = (end - 1) * h / p.maximum()[0] / 8;
                p.dirty(i, key[i], first) || p.dirty(i, key[i], last)
            });
            if dirty {
                if open.is_none() {
                    open = Some(x);
                }
            } else if let Some(start) = open.take() {
                self.source_runs.push(start..x);
            }
        }
        if let Some(start) = open {
            self.source_runs.push(start..p.width());
        }
        for run in &self.source_runs {
            let mut target = [NONE, 0];
            for map in &self.block_targets[run.start / 8..(run.end.div_ceil(8))] {
                target[0] = target[0].min(map[0]);
                target[1] = target[1].max(map[1]);
            }
            if target[0] != NONE {
                self.targets.push(target[0]..target[1]);
            }
        }
        self.targets.sort_unstable_by_key(|r| r.start);
        let mut used = 0usize;
        for index in 0..self.targets.len() {
            let r = self.targets[index].clone();
            if used != 0 && r.start <= self.targets[used - 1].end {
                self.targets[used - 1].end = self.targets[used - 1].end.max(r.end);
            } else {
                self.targets[used] = r;
                used += 1;
            }
        }
        self.targets.truncate(used);
        true
    }
    pub(super) fn ranges(&self) -> &[Range<usize>] {
        &self.targets
    }
    pub(super) fn row(
        &mut self,
        p: &Planar,
        y: usize,
        bands: &[Band],
        taps: &[Tap],
        out: &mut [[f32; 4]],
    ) -> bool {
        let Some(repeat) = p.chroma_repeat() else {
            return false;
        };
        if !self.prepare(p, y) {
            return false;
        }
        if !convert(p, y, repeat, &self.source_runs, &mut self.rgb) {
            return false;
        }
        out.fill([0.0; 4]);
        let pixels = &self.rgb;
        for range in &self.targets {
            for (out, band) in out[range.clone()].iter_mut().zip(&bands[range.clone()]) {
                let t = &taps[band.start..band.end];
                let a = t[0];
                let b = t[t.len() - 1];
                let first = pixels[a.sample];
                if a.sample == b.sample {
                    *out = first.map(f32::from);
                    continue;
                }
                let last = pixels[b.sample];
                let mut sum = [0u32; 4];
                for p in &pixels[a.sample + 1..b.sample] {
                    for c in 0..4 {
                        sum[c] = sum[c].wrapping_add(u32::from(p[c]));
                    }
                }
                let middle = if t.len() > 2 { t[1].weight } else { 0.0 };
                *out = std::array::from_fn(|c| {
                    sum[c] as f32 * middle
                        + f32::from(first[c]) * a.weight
                        + f32::from(last[c]) * b.weight
                });
            }
        }
        true
    }
}

fn convert(
    p: &Planar,
    y: usize,
    repeat: usize,
    runs: &[Range<usize>],
    rgb: &mut [[u16; 4]],
) -> bool {
    let ys = &p.row(0, y)[..p.width()];
    let cb = p.row(1, y);
    let cr = p.row(2, y);
    for run in runs {
        for (group, chunk) in ys[run.clone()].chunks(8).enumerate() {
            let start = run.start + group * 8;
            let len = chunk.len();
            let cs = start / repeat;
            let ce = (start + len).div_ceil(repeat);
            let out = &mut rgb[start..start + len];
            if uniform(&cb[cs..ce], 128) && uniform(&cr[cs..ce], 128) {
                for (pixel, &v) in out.iter_mut().zip(chunk) {
                    let d = 255 - u16::from(v);
                    *pixel = [d, d, d, 0];
                }
                continue;
            }
            if uniform(&cb[cs..ce], cb[cs])
                && uniform(&cr[cs..ce], cr[cs])
                && Constant::new(cb[cs], cr[cs]).is_some_and(|c| c.deficits(chunk, out))
            {
                continue;
            }
            // Integer Table 13 offsets per chroma pair; a green half tie
            // keeps the binary64 conversion below.
            if deficits(chunk, &cb[cs..ce], &cr[cs..ce], repeat, out) {
                continue;
            }
            let mut bytes = [0; 24];
            if !ycbcr8(
                chunk,
                &cb[cs..ce],
                &cr[cs..ce],
                repeat,
                &mut bytes[..len * 3],
            ) {
                return false;
            }
            for (out, rgb) in out.iter_mut().zip(bytes[..len * 3].as_chunks::<3>().0) {
                *out = [
                    255 - u16::from(rgb[0]),
                    255 - u16::from(rgb[1]),
                    255 - u16::from(rgb[2]),
                    0,
                ];
            }
        }
    }
    true
}
