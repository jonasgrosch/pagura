//! Exact byte-domain PDF Table 13 conversion using its decimal coefficients.
//! Integer luminance can be added after rounding a chroma contribution, except
//! at a green half tie where the reference's two f64 subtractions matter.
use super::sample;
use crate::deferred::offset;

pub(super) struct Rgb {
    offsets: [i32; 3],
    cb: i32,
    cr: i32,
    green_tie: bool,
}
impl Rgb {
    #[inline]
    pub(super) fn new(cb: u8, cr: u8) -> Self {
        let (red, green, blue, green_tie) = offset(cb, cr);
        Self {
            offsets: [red, green, blue].map(i32::from),
            cb: i32::from(cb) - 128,
            cr: i32::from(cr) - 128,
            green_tie,
        }
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "channel sums are clamped to bytes before casting"
    )]
    #[inline]
    pub(super) fn pixel(&self, y: u8) -> [u8; 3] {
        let mut rgb = self
            .offsets
            .map(|offset| (i32::from(y) + offset).clamp(0, 255) as u8);
        if self.green_tie {
            rgb[1] = sample(
                f64::from(y) - 0.344_136 * f64::from(self.cb) - 0.714_136 * f64::from(self.cr),
            );
        }
        rgb
    }
}

/// Pixels converted together: fixed lane arrays the compiler vectorizes.
const LANES: usize = 16;

/// The most bytes [`Offsets`] holds per luma column: three chroma and three
/// channel rows of `i16`, and a tie for every chroma pair, reserved once
/// per row so that pushing it never grows past the row's pairs.
pub(in crate::dct) const ROW_BYTES: usize = 6 * size_of::<i16>() + size_of::<(usize, u8, u8)>();

/// Table 13 offsets of one chroma row, replicated to its luma columns, so a
/// luma row converts as independent lanes of clamped additions. Pixels of a
/// chroma pair at a green half tie are converted again with its [`Rgb`].
#[derive(Default)]
pub(super) struct Offsets {
    chroma: [Vec<i16>; 3],
    channels: [Vec<i16>; 3],
    ties: Vec<(usize, u8, u8)>,
    repeat: usize,
}
impl Offsets {
    /// Replicates each chroma pair over `repeat` of `width` luma columns.
    pub(super) fn fill(&mut self, blue: &[u8], red: &[u8], repeat: usize, width: usize) {
        self.ties.clear();
        self.repeat = repeat;
        let count = width.div_ceil(repeat).min(blue.len()).min(red.len());
        let (blue, red) = (&blue[..count], &red[..count]);
        for channel in &mut self.chroma {
            channel.reserve_exact(count.saturating_sub(channel.len()));
            channel.resize(count, 0);
        }
        let [r, g, b] = &mut self.chroma;
        let groups = r
            .as_chunks_mut::<LANES>()
            .0
            .iter_mut()
            .zip(g.as_chunks_mut::<LANES>().0)
            .zip(b.as_chunks_mut::<LANES>().0)
            .zip(
                blue.as_chunks::<LANES>()
                    .0
                    .iter()
                    .zip(red.as_chunks::<LANES>().0),
            );
        let mut tied = false;
        for (((r, g), b), (blue, red)) in groups {
            tied |= chroma(blue, red, [r, g, b]);
        }
        let done = count / LANES * LANES;
        for (index, (&cb, &cr)) in blue.iter().zip(red).enumerate().skip(done) {
            let tie;
            (r[index], g[index], b[index], tie) = offset(cb, cr);
            tied |= tie;
        }
        if tied {
            self.ties.reserve_exact(count);
            for (index, (&cb, &cr)) in blue.iter().zip(red).enumerate() {
                if offset(cb, cr).3 {
                    self.ties.push((index * repeat, cb, cr));
                }
            }
        }
        for (channel, chroma) in self.channels.iter_mut().zip(&self.chroma) {
            channel.reserve_exact(width.saturating_sub(channel.len()));
            channel.resize(width, 0);
            match repeat {
                1 => replicate::<1>(chroma, channel),
                2 => replicate::<2>(chroma, channel),
                4 => replicate::<4>(chroma, channel),
                _ => {
                    for (run, &offset) in channel.chunks_mut(repeat).zip(chroma) {
                        run.fill(offset);
                    }
                }
            }
        }
    }

    /// Converts one luma row to interleaved RGB with byte-domain rounding.
    pub(super) fn convert(&self, luma: &[u8], output: &mut [u8]) {
        let [red, green, blue] = &self.channels;
        let (groups, rest) = output.as_chunks_mut::<3>().0.as_chunks_mut::<LANES>();
        for ((((out, luma), red), green), blue) in groups
            .iter_mut()
            .zip(luma.as_chunks::<LANES>().0)
            .zip(red.as_chunks::<LANES>().0)
            .zip(green.as_chunks::<LANES>().0)
            .zip(blue.as_chunks::<LANES>().0)
        {
            lanes(luma, [red, green, blue], out);
        }
        let done = groups.len() * LANES;
        let tail = luma.iter().zip(red).zip(green).zip(blue).skip(done);
        for (pixel, (((&y, &red), &green), &blue)) in rest.iter_mut().zip(tail) {
            *pixel = [red, green, blue].map(|offset| add(y, offset));
        }
        let pixels = output.as_chunks_mut::<3>().0;
        for &(start, cb, cr) in &self.ties {
            let rgb = Rgb::new(cb, cr);
            let end = (start + self.repeat).min(pixels.len());
            let (Some(run), Some(luma)) = (pixels.get_mut(start..end), luma.get(start..)) else {
                continue;
            };
            for (pixel, &y) in run.iter_mut().zip(luma) {
                *pixel = rgb.pixel(y);
            }
        }
    }
}

/// Offsets of one group of chroma pairs; true if any pair is a green tie.
fn chroma(blue: &[u8; LANES], red: &[u8; LANES], [r, g, b]: [&mut [i16; LANES]; 3]) -> bool {
    let mut tied = false;
    for i in 0..LANES {
        let tie;
        (r[i], g[i], b[i], tie) = offset(blue[i], red[i]);
        tied |= tie;
    }
    tied
}

/// Copies each chroma offset to its `REPEAT` luma columns.
fn replicate<const REPEAT: usize>(chroma: &[i16], channel: &mut [i16]) {
    let (runs, rest) = channel.as_chunks_mut::<REPEAT>();
    for (run, &offset) in runs.iter_mut().zip(chroma) {
        *run = [offset; REPEAT];
    }
    if let Some(&offset) = chroma.get(runs.len()) {
        rest.fill(offset);
    }
}

/// One group of pixels: clamped luma plus offset per channel, interleaved.
fn lanes(luma: &[u8; LANES], [red, green, blue]: [&[i16; LANES]; 3], out: &mut [[u8; 3]; LANES]) {
    for (i, pixel) in out.iter_mut().enumerate() {
        *pixel = [red[i], green[i], blue[i]].map(|offset| add(luma[i], offset));
    }
}

/// A clamped channel. The sum lies within -227..=482, so the wrapping
/// addition is exact and, free of overflow checks, vectorizes.
fn add(luma: u8, offset: i16) -> u8 {
    clamp(i16::from(luma).wrapping_add(offset))
}

#[expect(
    clippy::cast_sign_loss,
    reason = "the sum is clamped to the byte range"
)]
fn clamp(value: i16) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::{Offsets, ROW_BYTES, Rgb};

    #[test]
    fn a_row_of_ties_stays_within_its_admitted_bytes() {
        // Cb 178 and Cr 78 are a green half tie: every pair of the row ties.
        for (width, repeat) in [1_usize, 2, 3, 4, 17, 1025, 1027]
            .into_iter()
            .flat_map(|width| (1..=4).map(move |repeat| (width, repeat)))
        {
            let count = width.div_ceil(repeat);
            let mut offsets = Offsets::default();
            for _ in 0..3 {
                offsets.fill(&vec![178; count], &vec![78; count], repeat, width);
                assert_eq!(offsets.ties.len(), count);
                let held = offsets
                    .chroma
                    .iter()
                    .chain(&offsets.channels)
                    .map(|row| row.capacity() * size_of::<i16>())
                    .sum::<usize>()
                    + offsets.ties.capacity() * size_of::<(usize, u8, u8)>();
                assert!(held <= width * ROW_BYTES, "{width}/{repeat}: {held}");
            }
        }
    }

    #[test]
    fn lane_rows_match_each_pixel_and_green_tie() {
        // Every chroma pair, including the ties, at each replication and a
        // ragged final group, against the scalar conversion.
        let pairs: Vec<(u8, u8)> = (0..=255u8)
            .flat_map(|cb| (0..=255u8).map(move |cr| (cb, cr)))
            .collect();
        let mut offsets = Offsets::default();
        for repeat in 1..=4 {
            for (row, chunk) in pairs.chunks(97).enumerate() {
                let width = chunk.len() * repeat - usize::from(repeat > 1);
                let (blue, red): (Vec<u8>, Vec<u8>) = chunk.iter().copied().unzip();
                let luma: Vec<u8> = (0..width)
                    .map(|x| u8::try_from((x * 37 + row * 11) % 256).unwrap())
                    .collect();
                offsets.fill(&blue, &red, repeat, width);
                let mut output = vec![0; width * 3];
                offsets.convert(&luma, &mut output);
                for (x, (pixel, &y)) in output.as_chunks::<3>().0.iter().zip(&luma).enumerate() {
                    let rgb = Rgb::new(blue[x / repeat], red[x / repeat]);
                    assert_eq!(*pixel, rgb.pixel(y), "repeat {repeat} row {row} column {x}");
                }
            }
        }
    }

    #[test]
    fn every_byte_triplet_keeps_table_13_rounding() {
        let mut green_ties = 0;
        for cb in 0..=255u8 {
            for cr in 0..=255u8 {
                let rgb = Rgb::new(cb, cr);
                green_ties += usize::from(rgb.green_tie);
                for y in 0..=255u8 {
                    let exact = super::super::ycbcr(
                        f64::from(y),
                        f64::from(cb) - 128.0,
                        f64::from(cr) - 128.0,
                    )
                    .map(super::super::sample);
                    assert_eq!(rgb.pixel(y), exact, "{y} {cb} {cr}");
                }
            }
        }
        eprintln!(
            "exhaustive RGB: 16777216 triplets, {green_ties} chroma pairs require half-tie evaluation"
        );
    }
}
