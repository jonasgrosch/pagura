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

pub(super) fn decode(
    reader: &mut Reader<'_>,
    width: u32,
    height: u32,
    limits: &Limits,
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
        return super::mmr::decode(reader.rest(), width, height, limits, base)
            .map_err(|error| error.at(position));
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
    let mut bitmap = Bitmap::new(width, height, false, limits)?;
    let mut typical = 0;
    let typical_context = [0x9b25, 0x0795, 0xe5, 0x0195][usize::from(positions.kind)];
    for y in 0..height {
        if prediction {
            typical ^= decoder.bit(&mut contexts[typical_context])?;
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
            let value = decoder.bit(&mut contexts[context])?;
            bitmap.set(x, y, value);
        }
    }
    Ok(bitmap)
}
