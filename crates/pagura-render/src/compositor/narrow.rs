//! Normal premultiplied stores on the 0..255 level scale (11.3.3).
//! N16's single integer rounding contracts error below 128/257 levels;
//! F32 admission bounds accumulated rounding before this buffer is allocated.
use super::{Source, plan::Format};
use pagura_core::{Error, Result};

pub(super) enum Buffer {
    N16(Vec<[u16; 4]>),
    F32(Vec<[f32; 4]>),
}
impl Buffer {
    pub(super) fn new(format: Format, count: usize, background: [u8; 4]) -> Result<Self> {
        fn filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>> {
            let mut pixels = Vec::new();
            pixels
                .try_reserve_exact(count)
                .map_err(|_| Error::limit("colour buffer allocation failed"))?;
            pixels.resize(count, value);
            Ok(pixels)
        }
        match format {
            Format::N16 => Ok(Self::N16(filled(
                count,
                background.map(|v| u16::from(v) * 257),
            )?)),
            Format::F32 => Ok(Self::F32(filled(count, background.map(f32::from))?)),
            Format::F64 => Err(Error::invalid_argument("wide format in narrow buffer")),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::N16(v) => v.len() * 8,
            Self::F32(v) => v.len() * 16,
        }
    }
    pub(super) fn path(
        &mut self,
        offset: usize,
        coverage: &[u8],
        color: [f64; 4],
        alpha: f64,
    ) -> Result<()> {
        match self {
            Self::N16(pixels) => {
                let color = [word(color[0]), word(color[1]), word(color[2]), 65535];
                let row = pixels
                    .get_mut(offset..offset + coverage.len())
                    .ok_or(Error::invalid_argument("mask outside colour buffer"))?;
                for (pixel, &c) in row.iter_mut().zip(coverage) {
                    match c {
                        0 => {}
                        255 => *pixel = color,
                        c => {
                            for (out, source) in pixel.iter_mut().zip(color) {
                                *out = mix(source, *out, c);
                            }
                        }
                    }
                }
            }
            Self::F32(pixels) => {
                let color = [level(color[0]), level(color[1]), level(color[2]), 255.0];
                let k = scalar(alpha / 255.0);
                let row = pixels
                    .get_mut(offset..offset + coverage.len())
                    .ok_or(Error::invalid_argument("mask outside colour buffer"))?;
                for (pixel, &c) in row.iter_mut().zip(coverage) {
                    let a = f32::from(c) * k;
                    if c != 0 {
                        lerp(pixel, color, a);
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn paint(&mut self, index: usize, source: Source) -> Result<()> {
        let Self::F32(pixels) = self else {
            return Err(Error::invalid_argument("dynamic source in N16 buffer"));
        };
        let pixel = pixels
            .get_mut(index)
            .ok_or(Error::invalid_argument("source outside colour buffer"))?;
        let color = [
            level(source.color[0]),
            level(source.color[1]),
            level(source.color[2]),
            255.0,
        ];
        lerp(pixel, color, scalar(source.alpha));
        Ok(())
    }
    pub(super) fn output(&self, offset: usize, target: &mut [u8]) {
        match self {
            Self::N16(pixels) => {
                for (out, pixel) in target
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(&pixels[offset..])
                {
                    *out =
                        pixel.map(|v| u8::try_from((u32::from(v) + 128) / 257).unwrap_or(u8::MAX));
                }
            }
            Self::F32(pixels) => {
                for (out, pixel) in target
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(&pixels[offset..])
                {
                    *out = pixel.map(byte);
                }
            }
        }
    }
}
fn lerp(pixel: &mut [f32; 4], target: [f32; 4], alpha: f32) {
    if alpha == 1.0 {
        *pixel = target;
        return;
    }
    for (out, target) in pixel.iter_mut().zip(target) {
        *out += alpha * (target - *out);
    }
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "normalized source or alpha converted to the admitted storage precision"
)]
fn scalar(value: f64) -> f32 {
    value as f32
}
fn level(value: f64) -> f32 {
    let value = value.clamp(0.0, 1.0) * 255.0;
    let nearest = scalar(value);
    // Round inexact targets to an odd significand so a following nearest
    // byte store retains their side of an exactly representable half level.
    let truncated = nearest.to_bits() - u32::from(f64::from(nearest) > value);
    f32::from_bits(truncated | u32::from(f64::from(nearest) != value))
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite level rounded and clamped to a byte"
)]
fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "normalized colour rounded to 0..65535"
)]
fn word(value: f64) -> u16 {
    let level = value.clamp(0.0, 1.0) * 255.0;
    let word = (level * 257.0).round() as u16;
    let byte = (u32::from(word) + 128) / 257;
    let nearest = level.round() as u32;
    // Avoid double rounding at half levels. The adjacent word remains
    // within one word of the source, keeping the 128/257-level bound.
    match byte.cmp(&nearest) {
        std::cmp::Ordering::Greater => word - 1,
        std::cmp::Ordering::Less => word + 1,
        std::cmp::Ordering::Equal => word,
    }
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "convex combination of words stays within 0..65535"
)]
fn mix(source: u16, backdrop: u16, coverage: u8) -> u16 {
    let c = u32::from(coverage);
    ((u32::from(source) * c + u32::from(backdrop) * (255 - c) + 127) / 255) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(buffer: &Buffer) -> u8 {
        let mut out = [0; 4];
        buffer.output(0, &mut out);
        out[0]
    }
    #[test]
    fn rational_normal_stacks() {
        // Independent rational recurrences of 11.3.3: 218.7107843137,
        // 219.5003822059, 93.9936190601, 93.6250166324, 5.0127537023.
        for format in [Format::N16, Format::F32] {
            let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
            buffer.path(0, &[255], [0.85; 4], 1.0).unwrap();
            for _ in 0..2 {
                buffer.path(0, &[10], [0.95; 4], 1.0).unwrap();
            }
            assert_eq!(value(&buffer), 219);
            let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
            for (s, c) in [(159, 95), (240, 10), (80, 1)] {
                buffer
                    .path(0, &[c], [f64::from(s) / 255.0; 4], 1.0)
                    .unwrap();
            }
            assert_eq!(
                value(&buffer),
                if format == Format::N16 { 219 } else { 220 }
            );
            for (n, expected) in [(254, 94), (255, 94), (1000, 5), (270_000, 0)] {
                let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
                for _ in 0..n {
                    buffer.path(0, &[1], [0.0; 4], 1.0).unwrap();
                }
                assert_eq!(value(&buffer), expected, "{format:?} {n}");
            }
        }
    }
    #[test]
    fn translucent_stagnation_requires_the_format_bound() {
        let mut buffer = Buffer::new(Format::F32, 1, [255; 4]).unwrap();
        for _ in 0..20_000 {
            buffer.path(0, &[1], [10.6 / 255.0; 4], 0.5).unwrap();
        }
        assert_eq!(value(&buffer), 11);
        let mut buffer = Buffer::new(Format::F32, 1, [255; 4]).unwrap();
        for _ in 0..270_000 {
            buffer
                .path(0, &[255], [0.0; 4], 3.0 / 134_217_728.0)
                .unwrap();
        }
        // Exact closed form is 253.4657168936922. Forced F32 violates
        // the bound; the planner must keep this stack on F64.
        assert_eq!(value(&buffer), 255);
    }
    #[test]
    fn word_embedding_and_background_rounding_are_exact() {
        for w in 0..=65_535_u32 {
            let byte = (w + 128) / 257;
            let residual = i64::from(w) - i64::from(byte) * 257;
            assert!((-128..=128).contains(&residual));
            assert_eq!(i64::from(byte) * 257 + residual, i64::from(w));
        }
        for c in 0..=255_u32 {
            for a in 0..=255_u32 {
                let w = (c * a * 257 + 127) / 255;
                assert_eq!((w + 128) / 257, (c * a + 127) / 255);
            }
        }
    }
    #[test]
    fn source_encoding_preserves_byte_rounding_at_half_levels() {
        for format in [Format::N16, Format::F32] {
            for (source, expected) in [
                (0.5_f64.next_down(), 127),
                (0.5, 128),
                (0.5_f64.next_up(), 128),
            ] {
                let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
                buffer.path(0, &[255], [source; 4], 1.0).unwrap();
                assert_eq!(value(&buffer), expected);
            }
            for byte in 0..=255 {
                let mut buffer = Buffer::new(format, 1, [byte; 4]).unwrap();
                assert_eq!(value(&buffer), byte);
                buffer
                    .path(0, &[255], [f64::from(byte) / 255.0; 4], 1.0)
                    .unwrap();
                assert_eq!(value(&buffer), byte);
            }
        }
    }
    #[test]
    fn zero_coverage_is_a_noop_and_full_coverage_overwrites() {
        for format in [Format::N16, Format::F32] {
            let mut buffer = Buffer::new(format, 1, [42; 4]).unwrap();
            buffer.path(0, &[0], [0.0; 4], 1.0).unwrap();
            assert_eq!(value(&buffer), 42);
            buffer.path(0, &[255], [1.0; 4], 1.0).unwrap();
            assert_eq!(value(&buffer), 255);
        }
    }
    #[test]
    fn dynamic_store_and_memory_use_the_selected_format() {
        let n = Buffer::new(Format::N16, 3, [255; 4]).unwrap();
        assert_eq!(n.bytes(), 24);
        let mut f = Buffer::new(Format::F32, 3, [255; 4]).unwrap();
        assert_eq!(f.bytes(), 48);
        f.paint(0, Source::new([0.0; 4], 0.5, 1.0)).unwrap();
        let mut out = [0; 4];
        f.output(0, &mut out);
        assert_eq!(out, [128, 128, 128, 255]);
        assert!(Buffer::new(Format::F64, 1, [0; 4]).is_err());
    }
    #[test]
    fn sampled_and_binade_edge_stores_respect_the_error_envelope() {
        let mut seed = 7_u64;
        let mut random = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            u32::try_from(seed >> 32).unwrap()
        };
        for i in 0..100_000 {
            let x = if i % 3 == 0 {
                f32::from_bits(255.0_f32.to_bits() - (random() % 64))
            } else {
                level(f64::from(random()) / f64::from(u32::MAX))
            };
            let source = f64::from(random()) / f64::from(u32::MAX);
            let alpha = f64::from(random()) / f64::from(u32::MAX);
            let c = u8::try_from(random() % 256).unwrap();
            let a = f64::from(c) * alpha / 255.0;
            let mut pixel = [x; 4];
            lerp(
                &mut pixel,
                [level(source); 4],
                f32::from(c) * scalar(alpha / 255.0),
            );
            let ideal = f64::from(x) + a * (255.0 * source - f64::from(x));
            let bound = 1.0 / 131_072.0 + a * (2.0 / 131_072.0 + 255.0 * 6.0 / 16_777_216.0);
            assert!((f64::from(pixel[0]) - ideal).abs() <= bound + 1e-12);
        }
    }
}
