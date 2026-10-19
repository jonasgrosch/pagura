//! Generic regions (T.88 6.2 and 7.4.6).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::{Fill, Row, set},
    mmr::{self, Eofb},
};
use crate::budget::Budget;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};

// Context bits in increasing significance. Adaptive positions are inserted
// into the holes specified by T.88 Figures 3–6, rather than sorted by
// coordinates.
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

/// A generic region template, GBTEMPLATE (T.88 6.2.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GenericTemplate {
    Zero,
    One,
    Two,
    Three,
}

impl GenericTemplate {
    /// The template a two-bit field selects.
    pub(super) const fn from_two_bits(bits: u16) -> Self {
        match bits & 3 {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Two,
            _ => Self::Three,
        }
    }

    /// The template's pixels, nominal adaptive pixels included.
    const fn pixels(self) -> &'static [(i32, i32)] {
        match self {
            Self::Zero => &TEMPLATE_0,
            Self::One => &TEMPLATE_1,
            Self::Two => &TEMPLATE_2,
            Self::Three => &TEMPLATE_3,
        }
    }

    /// The indices of the adaptive pixels among [`GenericTemplate::pixels`],
    /// the first being A1.
    const fn adaptive(self) -> &'static [usize] {
        match self {
            Self::Zero => &[4, 10, 11, 15],
            Self::One => &[3],
            Self::Two => &[2],
            Self::Three => &[4],
        }
    }

    /// The context of the typical prediction bit SLTP (T.88 6.2.5.7).
    const fn typical_context(self) -> usize {
        match self {
            Self::Zero => 0x9b25,
            Self::One => 0x0795,
            Self::Two => 0x00e5,
            Self::Three => 0x0195,
        }
    }
}

/// Whether typical prediction is on: TPGDON for generic regions (T.88
/// 6.2.5.7), TPGRON for refinement regions (T.88 6.3.5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Prediction {
    Off,
    On,
}

impl Prediction {
    pub(super) const fn from_flag(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

/// Generic region segment flags (T.88 7.4.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GenericFlags(pub(super) u8);

impl GenericFlags {
    /// Bit 0: MMR coding.
    const MMR: u8 = 0x01;
    /// Bits 1–2: the template.
    const TEMPLATE_SHIFT: u32 = 1;
    /// Bit 3: typical prediction.
    const TYPICAL: u8 = 0x08;
    /// Bit 4: an extended template.
    const EXTENDED: u8 = 0x10;
    /// Bits 5–7: reserved.
    const RESERVED: u8 = 0xe0;
    /// The flags of arithmetic coding.
    const ARITHMETIC: u8 = 0x1e;

    const fn template(self) -> GenericTemplate {
        GenericTemplate::from_two_bits(u16::from_be_bytes([0, self.0 >> Self::TEMPLATE_SHIFT]))
    }

    pub(super) const fn mmr(self) -> bool {
        self.0 & Self::MMR != 0
    }

    /// The length of the adaptive pixel positions that follow the flags.
    pub(super) const fn adaptive_bytes(self) -> usize {
        if self.mmr() {
            0
        } else {
            2 * self.template().adaptive().len()
        }
    }
}

/// Decodes a generic region segment's data (T.88 7.4.6), whose MMR data
/// must end with an end-of-block code as `eofb` says.
pub(super) fn decode(
    cursor: &mut Cursor<'_>,
    size: (u32, u32),
    eofb: Eofb,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let (width, height) = size;
    let flags = GenericFlags(cursor.u8()?);
    if flags.0 & GenericFlags::RESERVED != 0 {
        return Err(Error::syntax("reserved JBIG2 generic-region flags"));
    }
    if flags.mmr() {
        if flags.0 & GenericFlags::ARITHMETIC != 0 {
            return Err(Error::syntax("arithmetic flags in JBIG2 MMR region"));
        }
        let bitmap = mmr::decode(cursor, size, eofb, limits, budget)?;
        if !cursor.at_end() {
            return Err(cursor.error("trailing JBIG2 MMR data"));
        }
        return Ok(bitmap);
    }
    if flags.0 & GenericFlags::EXTENDED != 0 {
        return Err(Error::unsupported(
            crate::features::JBIG2_EXTENDED_TEMPLATE,
            "extended JBIG2 adaptive templates are not implemented",
        ));
    }
    let template = Template::read(cursor, flags.template())?;
    let mut contexts = budget.filled(template.context_count(), Context::default())?;
    let mut bitmap = Bitmap::new(width, height, Fill::White, limits, budget)?;
    let mut decoder = Decoder::new(cursor)?;
    decode_into(
        &mut bitmap,
        &template,
        Prediction::from_flag(flags.0 & GenericFlags::TYPICAL != 0),
        &mut decoder,
        &mut contexts,
        None,
    );
    budget.release::<Context>(contexts.len());
    Ok(bitmap)
}

/// A template with its adaptive pixels placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Template {
    pixels: [(i32, i32); 16],
    template: GenericTemplate,
}

impl Template {
    /// Reads the adaptive pixels of `template` (T.88 7.4.6.3).
    pub(super) fn read(cursor: &mut Cursor<'_>, template: GenericTemplate) -> Result<Self> {
        let mut placed = Self::nominal(template);
        for &index in template.adaptive() {
            let x = i32::from(cursor.i8()?);
            let y = i32::from(cursor.i8()?);
            if y > 0 || (y == 0 && x >= 0) {
                return Err(Error::syntax("noncausal JBIG2 adaptive pixel"));
            }
            placed.pixels[index] = (x, y);
        }
        Ok(placed)
    }

    /// `template` with its nominal adaptive pixels, A1 moved to `(first, 0)`
    /// if given: the templates of pattern dictionaries (T.88 Table 27) and
    /// gray-scale planes (T.88 C.5).
    pub(super) fn prescribed(template: GenericTemplate, first: Option<i32>) -> Self {
        let mut placed = Self::nominal(template);
        if let (Some(x), Some(&index)) = (first, template.adaptive().first()) {
            placed.pixels[index] = (x, 0);
        }
        placed
    }

    fn nominal(template: GenericTemplate) -> Self {
        let mut pixels = [(0, 0); 16];
        for (pixel, &nominal) in pixels.iter_mut().zip(template.pixels()) {
            *pixel = nominal;
        }
        Self { pixels, template }
    }

    fn pixels(&self) -> &[(i32, i32)] {
        &self.pixels[..self.template.pixels().len()]
    }

    pub(super) fn context_count(&self) -> usize {
        1 << self.pixels().len()
    }
}

/// Decodes the pixels of `bitmap`, which must be white, with the generic
/// region decoding procedure (T.88 6.2.5), leaving the pixels set in `skip`
/// white.
///
/// The context of each pixel is read from the rows its template reaches,
/// which are looked up once per row.
pub(super) fn decode_into(
    bitmap: &mut Bitmap,
    template: &Template,
    prediction: Prediction,
    decoder: &mut Decoder<'_>,
    contexts: &mut [Context],
    skip: Option<&Bitmap>,
) {
    let (width, stride) = (bitmap.width, bitmap.stride);
    let pixels = template.pixels();
    let typical_context = template.template.typical_context();
    let mut typical = 0;
    for y in 0..bitmap.height {
        if prediction == Prediction::On {
            typical ^= decoder.bit(&mut contexts[typical_context]);
        }
        let Some((above, current)) = bitmap.split_row(y) else {
            return;
        };
        if typical != 0 {
            // A typical row repeats the row above it (T.88 6.2.5.7); the
            // first row stays white.
            if let Some(previous) = above.get(above.len().saturating_sub(stride)..)
                && previous.len() == current.len()
            {
                current.copy_from_slice(previous);
            }
            continue;
        }
        let mut rows = [Row::EMPTY; 16];
        for (row, &(_, dy)) in rows.iter_mut().zip(pixels) {
            *row = Row::of(above, stride, width, i64::from(y) + i64::from(dy));
        }
        let skip = skip.map(|skip| skip.row(i64::from(y)));
        for x in 0..width {
            if skip.is_some_and(|skip| skip.get(i64::from(x)) != 0) {
                continue;
            }
            let mut context = 0;
            for (bit, (&(dx, dy), &row)) in pixels.iter().zip(&rows).enumerate() {
                // Pixels of the current row come before `x`, already decoded.
                let row = if dy == 0 {
                    Row::new(current, width)
                } else {
                    row
                };
                context |= usize::from(row.get(i64::from(x) + i64::from(dx))) << bit;
            }
            let value = decoder.bit(&mut contexts[context]);
            set(current, u32_to_usize(x), value);
        }
    }
}
