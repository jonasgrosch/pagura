//! Footprints over byte samples with integer interior sums.
use super::footprint::{Band, Tap};

/// Gray footprint sums with word-wide interiors and a constant-run shortcut.
pub(super) fn row(source: &[u8], bands: &[Band], taps: &[Tap], factor: f32, output: &mut [f32]) {
    for (out, band) in output.iter_mut().zip(bands) {
        let t = &taps[band.start..band.end];
        let first = t[0];
        let last = t[t.len() - 1];
        let n = last.sample - first.sample + 1;
        let a = source[first.sample];
        if n == 1 {
            *out = f32::from(a) * factor;
            continue;
        }
        if n <= 8 && first.sample + 8 <= source.len() {
            let word = u64::from_le_bytes(
                source[first.sample..first.sample + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let mask = u64::MAX >> (8 * (8 - n));
            let word = word & mask;
            if word == (u64::from(a) * 0x0101_0101_0101_0101) & mask {
                *out = f32::from(a) * factor;
                continue;
            }
            let middle = if n > 2 {
                (word >> 8) & (u64::MAX >> (8 * (10 - n)))
            } else {
                0
            };
            let sum = (middle & 0x00ff_00ff_00ff_00ff) + ((middle >> 8) & 0x00ff_00ff_00ff_00ff);
            let sum = (sum & 0x0000_ffff_0000_ffff) + ((sum >> 16) & 0x0000_ffff_0000_ffff);
            let sum = (sum & 0xffff_ffff) + (sum >> 32);
            *out = (f32::from(a) * first.weight
                + sum as f32 * if n > 2 { t[1].weight } else { 0.0 }
                + f32::from(source[last.sample]) * last.weight)
                * factor;
        } else {
            let mut sum = 0.0;
            for tap in t {
                sum += f32::from(source[tap.sample]) * tap.weight;
            }
            *out = sum * factor;
        }
    }
}
