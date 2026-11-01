//! Generic arithmetic regions, T.88 clause 6.2 and clause 7.4.6.
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    reader::Reader,
};
use pagura_core::{Error, Limits, Result};

// Context bits in increasing significance. Adaptive positions are inserted
// into the holes specified by Figures 3–6, rather than sorted by coordinates.
const TEMPLATE_0: [(i32, i32); 16] = [
    (-1, 0),
    (-2, 0),
    (-3, 0),
    (-4, 0),
    (3, -1),
    (2, -1),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-2, -1),
    (-3, -1),
    (2, -2),
    (1, -2),
    (0, -2),
    (-1, -2),
    (-2, -2),
];
const TEMPLATE_1: [(i32, i32); 13] = [
    (-1, 0),
    (-2, 0),
    (-3, 0),
    (3, -1),
    (2, -1),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-2, -1),
    (2, -2),
    (1, -2),
    (0, -2),
    (-1, -2),
];
const TEMPLATE_2: [(i32, i32); 10] = [
    (-1, 0),
    (-2, 0),
    (2, -1),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-2, -1),
    (1, -2),
    (0, -2),
    (-1, -2),
];
const TEMPLATE_3: [(i32, i32); 10] = [
    (-1, 0),
    (-2, 0),
    (-3, 0),
    (-4, 0),
    (2, -1),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-2, -1),
    (-3, -1),
];

/// How a template's nominal pixels sit in rolling windows of rows `y`,
/// `y − 1` and `y − 2`. Bit `k` of a window holds the pixel `k` columns
/// left of its rightmost one: column `x − 1` for row `y`, `x + reach` for
/// the rows above. Per row, the window bits the template uses and their
/// shift into the context.
#[derive(Clone, Copy)]
struct Windows {
    reach: [usize; 2],
    masks: [u32; 3],
    shifts: [u32; 3],
}

const WINDOWS: [Windows; 4] = [
    Windows {
        reach: [3, 2],
        masks: [0xf, 0x7f, 0x1f],
        shifts: [0, 4, 11],
    },
    Windows {
        reach: [3, 2],
        masks: [0x7, 0x3f, 0xf],
        shifts: [0, 3, 9],
    },
    Windows {
        reach: [2, 1],
        masks: [0x3, 0x1f, 0x7],
        shifts: [0, 2, 7],
    },
    Windows {
        reach: [2, 0],
        masks: [0xf, 0x3f, 0],
        shifts: [0, 4, 0],
    },
];

/// Pixel `x` of a packed row; zero past its end.
#[inline]
fn pixel(row: &[u8], x: usize) -> u8 {
    row.get(x / 8).map_or(0, |byte| (byte >> (7 - x % 8)) & 1)
}

pub(super) fn decode(
    reader: &mut Reader<'_>,
    width: u32,
    height: u32,
    limits: &Limits,
    unknown: bool,
) -> Result<Bitmap> {
    let flags = reader.byte()?;
    if flags & 0xe0 != 0 {
        return Err(Error::syntax("reserved JBIG2 generic-region flags"));
    }
    if flags & 1 != 0 {
        if flags & 0x1e != 0 {
            return Err(Error::syntax("arithmetic flags in JBIG2 MMR region"));
        }
        let position = reader.position();
        let base = match position {
            pagura_core::Position::Buffer(p) => p,
            _ => 0,
        };
        let data = reader.rest();
        return if unknown {
            let (bitmap, consumed) =
                super::mmr::decode_prefix(data, width, height, limits, base, true)?;
            if consumed != data.len() {
                return Err(Error::syntax("trailing JBIG2 unknown-length MMR data"));
            }
            Ok(bitmap)
        } else {
            super::mmr::decode(data, width, height, limits, base)
                .map_err(|error| error.at(position))
        };
    }
    let template = (flags >> 1) & 3;
    if flags & 0x10 != 0 {
        return Err(Error::unsupported(
            super::tags::EXTENDED_TEMPLATE,
            "extended JBIG2 adaptive templates are not implemented",
        ));
    }
    let positions = Template::read(reader, template)?;
    let context_bytes = positions.context_bytes();
    let mut bitmap_limits = *limits;
    bitmap_limits.decoded_bytes = limits
        .decoded_bytes
        .checked_sub(context_bytes)
        .ok_or_else(|| Error::limit("JBIG2 arithmetic contexts exceed byte budget"))?;
    let mut contexts = vec![Context::default(); positions.context_count()];
    let base = match reader.position() {
        pagura_core::Position::Buffer(p) => p,
        _ => 0,
    };
    let mut decoder = Decoder::new(reader.rest(), base)?;
    decode_arithmetic(
        width,
        height,
        &positions,
        flags & 8 != 0,
        &mut decoder,
        &mut contexts,
        &bitmap_limits,
    )
}

pub(super) struct Template {
    positions: [(i32, i32); 16],
    kind: u8,
    count: usize,
}

impl Template {
    pub(super) fn read(reader: &mut Reader<'_>, template: u8) -> Result<Self> {
        let source: &[(i32, i32)] = match template {
            0 => &TEMPLATE_0,
            1 => &TEMPLATE_1,
            2 => &TEMPLATE_2,
            _ => &TEMPLATE_3,
        };
        let mut positions = [(0, 0); 16];
        positions[..source.len()].copy_from_slice(source);
        let adaptive: &[usize] = match template {
            0 => &[4, 10, 11, 15],
            1 => &[3],
            2 => &[2],
            _ => &[4],
        };
        for &index in adaptive {
            let x = reader.signed_byte()?;
            let y = reader.signed_byte()?;
            if y > 0 || (y == 0 && x >= 0) {
                return Err(Error::syntax("noncausal JBIG2 adaptive pixel"));
            }
            positions[index] = (x, y);
        }
        Ok(Self {
            positions,
            kind: template,
            count: source.len(),
        })
    }

    /// Prescribed adaptive pixels for patterns (Table 27) and gray planes (C.4).
    pub(super) fn prescribed(kind: u8, first: Option<i32>) -> Self {
        let source: &[(i32, i32)] = match kind {
            0 => &TEMPLATE_0,
            1 => &TEMPLATE_1,
            2 => &TEMPLATE_2,
            _ => &TEMPLATE_3,
        };
        let mut positions = [(0, 0); 16];
        positions[..source.len()].copy_from_slice(source);
        if let Some(x) = first {
            positions[match kind {
                1 => 3,
                2 => 2,
                _ => 4,
            }] = (x, 0);
        }
        Self {
            positions,
            kind,
            count: source.len(),
        }
    }

    fn nominal(&self) -> &'static [(i32, i32)] {
        match self.kind {
            0 => &TEMPLATE_0,
            1 => &TEMPLATE_1,
            2 => &TEMPLATE_2,
            _ => &TEMPLATE_3,
        }
    }

    pub(super) fn context_count(&self) -> usize {
        1usize << self.count
    }
    pub(super) fn context_bytes(&self) -> usize {
        self.context_count() * size_of::<Context>()
    }
}

pub(super) fn decode_arithmetic(
    width: u32,
    height: u32,
    positions: &Template,
    prediction: bool,
    decoder: &mut Decoder<'_>,
    contexts: &mut [Context],
    limits: &Limits,
) -> Result<Bitmap> {
    decode_skipping(
        (width, height),
        positions,
        prediction,
        decoder,
        contexts,
        None,
        limits,
    )
}

pub(super) fn decode_skipping(
    size: (u32, u32),
    positions: &Template,
    prediction: bool,
    decoder: &mut Decoder<'_>,
    contexts: &mut [Context],
    skip: Option<&Bitmap>,
    limits: &Limits,
) -> Result<Bitmap> {
    let (width, height) = size;
    let mut bitmap = Bitmap::new(width, height, false, limits)?;
    let mut typical = 0;
    let typical_context = [0x9b25, 0x0795, 0xe5, 0x0195][usize::from(positions.kind)];
    let windows = WINDOWS[usize::from(positions.kind & 3)];
    // Adaptive pixels away from their nominal places are read one by one,
    // in place of the window bit there.
    let mut moved = [(0u32, 0i64, 0i64); 4];
    let mut count = 0;
    let mut keep = u32::MAX;
    for ((bit, &at), &nominal) in (0u32..)
        .zip(&positions.positions[..positions.count])
        .zip(positions.nominal())
    {
        if at != nominal && count < moved.len() {
            moved[count] = (bit, i64::from(at.0), i64::from(at.1));
            count += 1;
            keep &= !(1 << bit);
        }
    }
    let moved = &moved[..count];
    let stride = bitmap.stride();
    for (y, at) in (0..height).zip(0usize..) {
        if prediction {
            typical ^= decoder.bit(&mut contexts[typical_context])?;
        }
        if typical != 0 {
            if at != 0 {
                let start = at * stride;
                bitmap.data.copy_within(start - stride..start, start);
            }
            continue;
        }
        let (done, rest) = bitmap.data.split_at_mut(at * stride);
        let row = &mut rest[..stride];
        let line = |dy: usize| {
            at.checked_sub(dy)
                .map_or(&[][..], |above| &done[above * stride..][..stride])
        };
        let above = [line(1), line(2)];
        // The windows at column 0; columns left of the row read as zeros.
        let mut held = [0u32; 3];
        for ((window, &reach), line) in held[1..].iter_mut().zip(&windows.reach).zip(above) {
            for x in 0..=reach {
                *window = (*window << 1) | u32::from(pixel(line, x));
            }
        }
        for (column, x) in (0..width).zip(0usize..) {
            let (column, line) = (i64::from(column), i64::from(y));
            let value = if skip.is_some_and(|s| s.pixel(column, line) != 0) {
                0
            } else {
                let mut context = held
                    .iter()
                    .zip(windows.masks.iter().zip(&windows.shifts))
                    .fold(0, |context, (&held, (&mask, &shift))| {
                        context | (held & mask) << shift
                    });
                if !moved.is_empty() {
                    context &= keep;
                    for &(bit, dx, dy) in moved {
                        let source: &[u8] = match usize::try_from(line + dy) {
                            Ok(source) if source == at => row,
                            Ok(source) => done.get(source * stride..).map_or(&[], |r| &r[..stride]),
                            Err(_) => &[],
                        };
                        let value = usize::try_from(column + dx).map_or(0, |c| pixel(source, c));
                        context |= u32::from(value) << bit;
                    }
                }
                let value = decoder.bit(&mut contexts[context as usize])?;
                row[x / 8] |= value << (7 - x % 8);
                value
            };
            held[0] = (held[0] << 1) | u32::from(value);
            held[1] = (held[1] << 1) | u32::from(pixel(above[0], x + 1 + windows.reach[0]));
            held[2] = (held[2] << 1) | u32::from(pixel(above[1], x + 1 + windows.reach[1]));
        }
    }
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use super::{Bitmap, Context, Decoder, Template, WINDOWS, decode_skipping};
    use pagura_core::Limits;

    /// Every nominal pixel's window bit lands on its context bit.
    #[test]
    fn windows_hold_each_template_pixel_at_its_context_bit() {
        for kind in 0..4u8 {
            let template = Template::prescribed(kind, None);
            let windows = WINDOWS[usize::from(kind)];
            let mut covered = 0u32;
            for (bit, &(dx, dy)) in (0u32..).zip(template.nominal()) {
                let row = usize::try_from(-dy).unwrap();
                let reach = if row == 0 {
                    -1
                } else {
                    i32::try_from(windows.reach[row - 1]).unwrap()
                };
                let k = u32::try_from(reach - dx).unwrap();
                assert_ne!(
                    windows.masks[row] & (1 << k),
                    0,
                    "template {kind} bit {bit}"
                );
                assert_eq!(windows.shifts[row] + k, bit, "template {kind} bit {bit}");
                covered |= 1 << bit;
            }
            let held: u32 = windows.masks.iter().map(|m| m.count_ones()).sum();
            assert_eq!(held, covered.count_ones(), "template {kind}");
        }
    }

    /// The context of every pixel read pixel by pixel, as T.88 6.2.5.3
    /// states it.
    fn reference(
        (width, height): (u32, u32),
        positions: &Template,
        prediction: bool,
        decoder: &mut Decoder<'_>,
        contexts: &mut [Context],
    ) -> Bitmap {
        let mut bitmap = Bitmap::new(width, height, false, &Limits::default()).unwrap();
        let mut typical = 0;
        let typical_context = [0x9b25, 0x0795, 0xe5, 0x0195][usize::from(positions.kind)];
        for y in 0..height {
            if prediction {
                typical ^= decoder.bit(&mut contexts[typical_context]).unwrap();
            }
            if typical != 0 {
                if y != 0 {
                    let stride = bitmap.stride();
                    let start = y as usize * stride;
                    bitmap.data.copy_within(start - stride..start, start);
                }
                continue;
            }
            for x in 0..width {
                let mut context = 0;
                for (bit, &(dx, dy)) in positions.positions[..positions.count].iter().enumerate() {
                    context |= usize::from(
                        bitmap.pixel(i64::from(x) + i64::from(dx), i64::from(y) + i64::from(dy)),
                    ) << bit;
                }
                let value = decoder.bit(&mut contexts[context]).unwrap();
                bitmap.set(x, y, value);
            }
        }
        bitmap
    }

    /// Rolling windows decode what per-pixel contexts decode, for every
    /// template with nominal and moved adaptive pixels, with and without
    /// typical prediction, on pseudo-random coded data.
    #[test]
    fn windowed_contexts_decode_as_per_pixel_contexts() {
        let mut state = 0x2545_f491_u32;
        let mut next = || {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16).to_le_bytes()[0]
        };
        for kind in 0..4u8 {
            for moved in [None, Some(-5), Some(-1)] {
                for prediction in [false, true] {
                    for (width, height) in [(37, 23), (8, 5), (130, 9)] {
                        let mut data: Vec<u8> = (0..600).map(|_| next().min(0xfe)).collect();
                        // Skewed bytes keep long runs of one decision.
                        for byte in data.iter_mut().step_by(3) {
                            *byte &= 0x0f;
                        }
                        data.extend([0xff, 0xac]);
                        let mut template = Template::prescribed(kind, moved);
                        if moved == Some(-1) {
                            // Moved upwards too, beyond the windows.
                            template.positions[0] = (1, -3);
                        }
                        let count = template.context_count();
                        let (mut a, mut b) = (
                            vec![Context::default(); count],
                            vec![Context::default(); count],
                        );
                        let mut first = Decoder::new(&data, 0).unwrap();
                        let mut second = Decoder::new(&data, 0).unwrap();
                        let expected =
                            reference((width, height), &template, prediction, &mut first, &mut a);
                        let actual = decode_skipping(
                            (width, height),
                            &template,
                            prediction,
                            &mut second,
                            &mut b,
                            None,
                            &Limits::default(),
                        )
                        .unwrap();
                        assert_eq!(
                            actual.data, expected.data,
                            "template {kind} {moved:?} {prediction} {width}x{height}"
                        );
                    }
                }
            }
        }
    }
}
