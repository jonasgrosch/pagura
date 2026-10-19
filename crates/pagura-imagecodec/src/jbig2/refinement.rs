//! Refinement bitmap coding (T.88 6.3 and 7.4.7).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::{Fill, Row, charge_pixels, set},
    generic::Prediction,
};
use crate::budget::Budget;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};

/// A refinement template, GRTEMPLATE (T.88 6.3.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefinementTemplate {
    /// Thirteen context pixels, two of them adaptive.
    Zero,
    /// Ten context pixels.
    One,
}

impl RefinementTemplate {
    pub(super) const fn from_flag(one: bool) -> Self {
        if one { Self::One } else { Self::Zero }
    }
}

/// Refinement region segment flags (T.88 7.4.7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RefinementFlags(pub(super) u8);

impl RefinementFlags {
    /// Bit 0: the template.
    const TEMPLATE: u8 = 0x01;
    /// Bit 1: typical prediction.
    const TYPICAL: u8 = 0x02;

    pub(super) const fn is_valid(self) -> bool {
        self.0 & !(Self::TEMPLATE | Self::TYPICAL) == 0
    }

    pub(super) const fn template(self) -> RefinementTemplate {
        RefinementTemplate::from_flag(self.0 & Self::TEMPLATE != 0)
    }

    pub(super) const fn prediction(self) -> Prediction {
        Prediction::from_flag(self.0 & Self::TYPICAL != 0)
    }
}

/// A refinement template with its adaptive pixels placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Parameters {
    template: RefinementTemplate,
    adaptive: [(i32, i32); 2],
}

impl Parameters {
    /// Reads the adaptive pixels of `template`, if it has any (T.88
    /// 7.4.7.3).
    pub(super) fn read(cursor: &mut Cursor<'_>, template: RefinementTemplate) -> Result<Self> {
        let mut adaptive = [(-1, -1); 2];
        if template == RefinementTemplate::Zero {
            for point in &mut adaptive {
                *point = (i32::from(cursor.i8()?), i32::from(cursor.i8()?));
            }
            let (x, y) = adaptive[0];
            if y > 0 || (y == 0 && x >= 0) {
                return Err(Error::syntax("noncausal JBIG2 refinement adaptive pixel"));
            }
        }
        Ok(Self { template, adaptive })
    }

    const fn context_count(self) -> usize {
        match self.template {
            RefinementTemplate::Zero => 1 << 13,
            RefinementTemplate::One => 1 << 10,
        }
    }

    /// The pixels of the bitmap being decoded that the template reads,
    /// relative to the decoded pixel (T.88 6.3.5.3).
    const fn output_pixels(self) -> [(i32, i32); 4] {
        match self.template {
            RefinementTemplate::Zero => [(-1, 0), (1, -1), (0, -1), self.adaptive[0]],
            RefinementTemplate::One => [(-1, 0), (1, -1), (0, -1), (-1, -1)],
        }
    }

    /// The pixels of the reference that the template reads, relative to
    /// the reference pixel under the decoded one, and how many there are.
    const fn reference_pixels(self) -> ([(i32, i32); 9], usize) {
        match self.template {
            RefinementTemplate::Zero => (
                [
                    (1, 1),
                    (0, 1),
                    (-1, 1),
                    (1, 0),
                    (0, 0),
                    (-1, 0),
                    (1, -1),
                    (0, -1),
                    self.adaptive[1],
                ],
                9,
            ),
            RefinementTemplate::One => (
                [
                    (1, 1),
                    (0, 1),
                    (1, 0),
                    (0, 0),
                    (-1, 0),
                    (0, -1),
                    (0, 0),
                    (0, 0),
                    (0, 0),
                ],
                6,
            ),
        }
    }

    /// The context of the typical prediction bit LTP (T.88 6.3.5.6).
    const fn typical_context(self) -> usize {
        match self.template {
            RefinementTemplate::Zero => 0x0010,
            RefinementTemplate::One => 0x0008,
        }
    }
}

pub(super) struct Model {
    parameters: Parameters,
    pub(super) contexts: Vec<Context>,
}

impl Model {
    /// A model with fresh contexts, or with a copy of `retained` ones.
    pub(super) fn new(
        parameters: Parameters,
        retained: Option<&[Context]>,
        budget: &mut Budget,
    ) -> Result<Self> {
        let count = parameters.context_count();
        if retained.is_some_and(|retained| retained.len() != count) {
            return Err(Error::syntax(
                "incompatible JBIG2 retained refinement contexts",
            ));
        }
        let mut contexts = budget.filled(count, Context::default())?;
        if let Some(retained) = retained {
            contexts.copy_from_slice(retained);
        }
        Ok(Self {
            parameters,
            contexts,
        })
    }

    /// Drops the model, releasing its contexts.
    pub(super) fn release(self, budget: &mut Budget) {
        budget.release::<Context>(self.contexts.len());
    }

    pub(super) fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        reference: &Bitmap,
        request: Request,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        self.decode_reference(
            decoder,
            Reference::whole(reference),
            request,
            limits,
            budget,
        )
    }

    /// A page reference is restricted to the region rectangle (T.88
    /// 7.4.7.4).
    pub(super) fn decode_page(
        &mut self,
        decoder: &mut Decoder<'_>,
        page: &Bitmap,
        origin: (u32, u32),
        request: Request,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        self.decode_reference(
            decoder,
            Reference {
                bitmap: page,
                x: i64::from(origin.0),
                y: i64::from(origin.1),
                width: request.width,
                height: request.height,
            },
            request,
            limits,
            budget,
        )
    }

    fn decode_reference(
        &mut self,
        decoder: &mut Decoder<'_>,
        reference: Reference<'_>,
        request: Request,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let Request {
            width,
            height,
            dx,
            dy,
            prediction,
        } = request;
        // Prediction inspects another 3x3 reference neighbourhood per pixel.
        charge_pixels(
            budget,
            (u64::from(width) * u64::from(height)).saturating_mul(2),
        )?;
        let mut bitmap = Bitmap::new(width, height, Fill::White, limits, budget)?;
        let stride = bitmap.stride;
        let output = self.parameters.output_pixels();
        let (pixels, count) = self.parameters.reference_pixels();
        let pixels = pixels.get(..count).unwrap_or_default();
        let typical_context = self.parameters.typical_context();
        let mut typical = 0;
        for y in 0..height {
            if prediction == Prediction::On {
                typical ^= decoder.bit(&mut self.contexts[typical_context]);
            }
            let ry = i64::from(y) - dy;
            let neighbourhood = [
                reference.row(ry - 1),
                reference.row(ry),
                reference.row(ry + 1),
            ];
            let mut reference_rows = [ReferenceRow::EMPTY; 9];
            for (row, &(_, oy)) in reference_rows.iter_mut().zip(pixels) {
                *row = reference.row(ry + i64::from(oy));
            }
            let Some((above, current)) = bitmap.split_row(y) else {
                break;
            };
            let mut output_rows = [Row::EMPTY; 4];
            for (row, &(_, oy)) in output_rows.iter_mut().zip(&output) {
                *row = Row::of(above, stride, width, i64::from(y) + i64::from(oy));
            }
            for x in 0..width {
                let rx = i64::from(x) - dx;
                let implicit = if typical == 0 {
                    None
                } else {
                    uniform(&neighbourhood, rx)
                };
                let value = if let Some(value) = implicit {
                    value
                } else {
                    let mut context = 0;
                    for (bit, (&(ox, oy), &row)) in output.iter().zip(&output_rows).enumerate() {
                        // Pixels of the current row come before `x`.
                        let row = if oy == 0 {
                            Row::new(current, width)
                        } else {
                            row
                        };
                        context |= usize::from(row.get(i64::from(x) + i64::from(ox))) << bit;
                    }
                    for (bit, (&(ox, _), row)) in pixels.iter().zip(reference_rows).enumerate() {
                        context |= usize::from(row.get(rx + i64::from(ox))) << (output.len() + bit);
                    }
                    decoder.bit(&mut self.contexts[context])
                };
                set(current, u32_to_usize(x), value);
            }
        }
        Ok(bitmap)
    }
}

/// A clipped, borrowed reference avoids copying an entire page rectangle.
#[derive(Clone, Copy)]
struct Reference<'a> {
    bitmap: &'a Bitmap,
    x: i64,
    y: i64,
    width: u32,
    height: u32,
}

impl<'a> Reference<'a> {
    fn whole(bitmap: &'a Bitmap) -> Self {
        Self {
            bitmap,
            x: 0,
            y: 0,
            width: bitmap.width,
            height: bitmap.height,
        }
    }

    /// Row `y` of the reference rectangle, empty outside it.
    fn row(&self, y: i64) -> ReferenceRow<'a> {
        if y < 0 || y >= i64::from(self.height) {
            return ReferenceRow::EMPTY;
        }
        ReferenceRow {
            row: self.bitmap.row(self.y + y),
            x: self.x,
            width: i64::from(self.width),
        }
    }
}

/// A row of a reference, clipped to the reference rectangle.
#[derive(Clone, Copy)]
struct ReferenceRow<'a> {
    row: Row<'a>,
    x: i64,
    width: i64,
}

impl ReferenceRow<'_> {
    const EMPTY: Self = Self {
        row: Row::EMPTY,
        x: 0,
        width: 0,
    };

    fn get(self, x: i64) -> u8 {
        if x < 0 || x >= self.width {
            0
        } else {
            self.row.get(self.x + x)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) dx: i64,
    pub(super) dy: i64,
    pub(super) prediction: Prediction,
}

/// The value of the 3×3 reference neighbourhood around `x` in `rows`, if
/// it is uniform (T.88 6.3.5.6).
fn uniform(rows: &[ReferenceRow<'_>; 3], x: i64) -> Option<u8> {
    let value = rows[1].get(x);
    rows.iter()
        .all(|row| (-1..=1).all(|dx| row.get(x + dx) == value))
        .then_some(value)
}
