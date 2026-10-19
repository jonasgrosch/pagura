//! Pattern dictionaries (T.88 6.7 and 7.4.4) and halftone regions (T.88 6.6
//! and 7.4.5).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::{self, CombinationOp, Fill, charge_pixels, set},
    generic::{self, GenericTemplate, Prediction, Template},
    mmr::{self, Eofb},
};
use crate::budget::Budget;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};

/// Pattern dictionary segment flags (T.88 7.4.4.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PatternFlags(u8);

impl PatternFlags {
    /// Bit 0, HDMMR: MMR coding.
    const MMR: u8 = 0x01;
    /// Bits 1–2, HDTEMPLATE: the template of arithmetic coding.
    const TEMPLATE: u8 = 0x06;
    const TEMPLATE_SHIFT: u32 = 1;

    const fn mmr(self) -> bool {
        self.0 & Self::MMR != 0
    }

    const fn is_valid(self) -> bool {
        // MMR coding has no template.
        self.0 & !(Self::MMR | Self::TEMPLATE) == 0 && (!self.mmr() || self.0 & Self::TEMPLATE == 0)
    }

    const fn template(self) -> GenericTemplate {
        GenericTemplate::from_two_bits(u16::from_be_bytes([0, self.0 >> Self::TEMPLATE_SHIFT]))
    }
}

/// Halftone region segment flags (T.88 7.4.5.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HalftoneFlags(u8);

impl HalftoneFlags {
    /// Bit 0, HMMR: MMR coding.
    const MMR: u8 = 0x01;
    /// Bits 1–2, HTEMPLATE: the template of arithmetic coding.
    const TEMPLATE: u8 = 0x06;
    const TEMPLATE_SHIFT: u32 = 1;
    /// Bit 3, HENABLESKIP: cells whose pattern lies outside the region are
    /// skipped.
    const SKIP: u8 = 0x08;
    /// Bits 4–6, HCOMBOP.
    const OPERATOR_SHIFT: u32 = 4;
    const OPERATOR: u8 = 0x07;
    /// Bit 7, HDEFPIXEL.
    const DEFAULT_PIXEL: u8 = 0x80;

    const fn mmr(self) -> bool {
        self.0 & Self::MMR != 0
    }

    const fn skip(self) -> bool {
        self.0 & Self::SKIP != 0
    }

    const fn template(self) -> GenericTemplate {
        GenericTemplate::from_two_bits(u16::from_be_bytes([0, self.0 >> Self::TEMPLATE_SHIFT]))
    }

    /// The operator, or `None` for a reserved one or for MMR coding with a
    /// template or skipping, which MMR does not have.
    fn operator(self) -> Option<CombinationOp> {
        if self.mmr() && self.0 & (Self::TEMPLATE | Self::SKIP) != 0 {
            return None;
        }
        CombinationOp::try_from((self.0 >> Self::OPERATOR_SHIFT) & Self::OPERATOR).ok()
    }
}

pub(super) struct Dictionary {
    /// At least one pattern, each `width × height` (T.88 6.7.5).
    patterns: Vec<Bitmap>,
    width: u32,
    height: u32,
}
pub(super) fn dictionary(
    cursor: &mut Cursor<'_>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Dictionary> {
    let flags = PatternFlags(cursor.u8()?);
    if !flags.is_valid() {
        return Err(Error::syntax("invalid JBIG2 pattern dictionary flags"));
    }
    let pattern_width = cursor.u8()?;
    let width = u32::from(pattern_width);
    let height = u32::from(cursor.u8()?);
    let last = cursor.u32()?;
    if width == 0 || height == 0 {
        return Err(Error::syntax("empty JBIG2 pattern"));
    }
    // The patterns lie side by side in one collective bitmap (T.88 6.7.5).
    let collective_width = u32::try_from((u64::from(last) + 1) * u64::from(width))
        .map_err(|_| Error::limit("JBIG2 pattern width overflow"))?;
    let count = collective_width / width;
    budget.work(u32_to_usize(count))?;
    charge_pixels(
        budget,
        (u64::from(collective_width) * u64::from(height)).saturating_mul(2),
    )?;
    bitmap::layout(collective_width, height, limits)?;
    let mut patterns = Vec::new();
    budget.reserve(&mut patterns, u32_to_usize(count))?;
    // A1 lies one pattern to the left (T.88 Table 27).
    let template = Template::prescribed(flags.template(), Some(-i32::from(pattern_width)));
    let size = (collective_width, height);
    let collective = if flags.mmr() {
        let collective = mmr::decode(cursor, size, Eofb::Optional, limits, budget)?;
        if !cursor.at_end() {
            return Err(cursor.error("trailing JBIG2 MMR data"));
        }
        collective
    } else {
        let mut contexts = budget.filled(template.context_count(), Context::default())?;
        let mut collective = Bitmap::new(collective_width, height, Fill::White, limits, budget)?;
        let mut decoder = Decoder::new(cursor)?;
        generic::decode_into(
            &mut collective,
            &template,
            Prediction::Off,
            &mut decoder,
            &mut contexts,
            None,
        );
        budget.release::<Context>(contexts.len());
        collective
    };
    for n in 0..count {
        let mut pattern = Bitmap::new(width, height, Fill::White, limits, budget)?;
        for y in 0..i64::from(height) {
            let source = collective.row(y);
            if let Some(target) = pattern.row_mut(y) {
                for x in 0..width {
                    set(
                        target,
                        u32_to_usize(x),
                        source.get(i64::from(n * width + x)),
                    );
                }
            }
        }
        patterns.push(pattern);
    }
    collective.release(budget);
    Ok(Dictionary {
        patterns,
        width,
        height,
    })
}

struct Grid {
    width: u32,
    height: u32,
    x: i64,
    y: i64,
    rx: i64,
    ry: i64,
}
impl Grid {
    fn position(&self, n: u32, m: u32) -> (i64, i64) {
        (
            (self.x + i64::from(m) * self.ry + i64::from(n) * self.rx) >> 8,
            (self.y + i64::from(m) * self.rx - i64::from(n) * self.ry) >> 8,
        )
    }
}

pub(super) fn region(
    cursor: &mut Cursor<'_>,
    size: (u32, u32),
    dictionary: &Dictionary,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let flags = HalftoneFlags(cursor.u8()?);
    let operator = flags
        .operator()
        .ok_or_else(|| Error::syntax("invalid JBIG2 halftone flags"))?;
    let grid = Grid {
        width: cursor.u32()?,
        height: cursor.u32()?,
        x: i64::from(cursor.i32()?),
        y: i64::from(cursor.i32()?),
        rx: i64::from(cursor.u16()?),
        ry: i64::from(cursor.u16()?),
    };
    let cells = u64::from(grid.width) * u64::from(grid.height);
    let bpp = dictionary.patterns.len().saturating_sub(1).bit_width();
    charge_pixels(
        budget,
        (u64::from(grid.width.max(1)) * u64::from(grid.height))
            .checked_mul(u64::from(bpp + 1))
            .ok_or_else(|| Error::limit("JBIG2 gray-scale work overflow"))?,
    )?;
    bitmap::layout(grid.width, grid.height, limits)?;
    let template = Template::prescribed(flags.template(), None);
    let skip = if flags.skip() {
        let mut skip = Bitmap::new(grid.width, grid.height, Fill::White, limits, budget)?;
        for m in 0..grid.height {
            for n in 0..grid.width {
                let (x, y) = grid.position(n, m);
                skip.set(
                    n,
                    m,
                    u8::from(
                        x + i64::from(dictionary.width) <= 0
                            || x >= i64::from(size.0)
                            || y + i64::from(dictionary.height) <= 0
                            || y >= i64::from(size.1),
                    ),
                );
            }
        }
        Some(skip)
    } else {
        None
    };
    let planes = if flags.mmr() {
        Planes::Mmr
    } else {
        Planes::Arithmetic(&template)
    };
    let indices = gray(cursor, &grid, planes, skip.as_ref(), bpp, limits, budget)?;
    if let Some(skip) = skip {
        skip.release(budget);
    }
    let fill = Fill::from_flag(flags.0 & HalftoneFlags::DEFAULT_PIXEL != 0);
    let mut output = Bitmap::new(size.0, size.1, fill, limits, budget)?;
    budget.work(indices.len())?;
    // Charge every candidate pattern, including fully clipped placements.
    charge_pixels(
        budget,
        cells
            .checked_mul(u64::from(dictionary.width) * u64::from(dictionary.height))
            .ok_or_else(|| Error::limit("JBIG2 halftone drawing work overflow"))?,
    )?;
    let columns = u32_to_usize(grid.width).max(1);
    for (m, row) in (0..grid.height).zip(indices.chunks(columns)) {
        for (n, &index) in (0..grid.width).zip(row) {
            let pattern = dictionary.patterns.get(index).ok_or_else(|| {
                Error::syntax("JBIG2 gray-scale index exceeds pattern dictionary")
            })?;
            let (x, y) = grid.position(n, m);
            output.combine_signed(pattern, x, y, operator);
        }
    }
    budget.release::<usize>(indices.len());
    Ok(output)
}

/// How the gray-scale bitplanes are coded.
#[derive(Clone, Copy)]
enum Planes<'a> {
    Mmr,
    Arithmetic(&'a Template),
}

/// The state that arithmetic-coded bitplanes share.
struct Arithmetic<'t, 'd> {
    template: &'t Template,
    contexts: Vec<Context>,
    decoder: Decoder<'d>,
}

/// Gray-coded bitplanes share arithmetic statistics (T.88 C.5 and 7.4.5.2).
fn gray(
    cursor: &mut Cursor<'_>,
    grid: &Grid,
    planes: Planes<'_>,
    skip: Option<&Bitmap>,
    bpp: u32,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Vec<usize>> {
    let cell_count = usize::try_from(u64::from(grid.width) * u64::from(grid.height))
        .map_err(|_| Error::limit("JBIG2 gray-scale cell count overflow"))?;
    let mut indices = budget.filled(cell_count, 0_usize)?;
    if bpp == 0 {
        if !cursor.at_end() {
            return Err(cursor.error("trailing JBIG2 gray-scale data"));
        }
        return Ok(indices);
    }
    let mut arithmetic = match planes {
        Planes::Mmr => None,
        Planes::Arithmetic(template) => Some(Arithmetic {
            template,
            contexts: budget.filled(template.context_count(), Context::default())?,
            decoder: Decoder::new(cursor)?,
        }),
    };
    let size = (grid.width, grid.height);
    let columns = u32_to_usize(grid.width).max(1);
    let mut previous: Option<Bitmap> = None;
    for bit in (0..bpp).rev() {
        let mut plane = match &mut arithmetic {
            None => mmr::decode(cursor, size, Eofb::Required, limits, budget)?,
            Some(arithmetic) => {
                let mut plane = Bitmap::new(grid.width, grid.height, Fill::White, limits, budget)?;
                generic::decode_into(
                    &mut plane,
                    arithmetic.template,
                    Prediction::Off,
                    &mut arithmetic.decoder,
                    &mut arithmetic.contexts,
                    skip,
                );
                plane
            }
        };
        // Each plane is coded as its exclusive or with the plane above it
        // (T.88 C.5); the zero padding of both stays zero.
        if let Some(previous) = &previous {
            for (byte, &above) in plane.data.iter_mut().zip(&previous.data) {
                *byte ^= above;
            }
        }
        for (m, cells) in (0..grid.height).zip(indices.chunks_mut(columns)) {
            let row = plane.row(i64::from(m));
            for (n, cell) in (0..).zip(cells) {
                *cell |= usize::from(row.get(n)) << bit;
            }
        }
        if let Some(previous) = previous.replace(plane) {
            previous.release(budget);
        }
    }
    if !cursor.at_end() {
        return Err(cursor.error("trailing JBIG2 gray-scale data"));
    }
    if let Some(previous) = previous {
        previous.release(budget);
    }
    if let Some(arithmetic) = arithmetic {
        budget.release::<Context>(arithmetic.contexts.len());
    }
    Ok(indices)
}
