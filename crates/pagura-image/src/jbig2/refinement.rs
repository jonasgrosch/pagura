//! Refinement bitmap coding. T.88 primary-text review is pending; geometric
//! vectors independently encoded in Python are compared exactly with PDFium.
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    reader::Reader,
};
use pagura_core::{Error, Limits, Result};

#[derive(Clone, Copy)]
pub(super) struct Parameters {
    kind: u8,
    adaptive: [(i32, i32); 2],
}

impl Parameters {
    pub(super) fn read(reader: &mut Reader<'_>, kind: u8) -> Result<Self> {
        let mut adaptive = [(-1, -1); 2];
        if kind == 0 {
            for point in &mut adaptive {
                *point = (reader.signed_byte()?, reader.signed_byte()?);
            }
            let (x, y) = adaptive[0];
            if y > 0 || (y == 0 && x >= 0) {
                return Err(Error::syntax("noncausal JBIG2 refinement adaptive pixel"));
            }
        }
        Ok(Self { kind, adaptive })
    }

    pub(super) fn context_bytes(self) -> usize {
        self.context_count() * size_of::<Context>()
    }
    fn context_count(self) -> usize {
        if self.kind == 0 { 8192 } else { 1024 }
    }
}

pub(super) struct Model {
    parameters: Parameters,
    pub(super) contexts: Vec<Context>,
}

impl Model {
    pub(super) fn new(
        parameters: Parameters,
        retained: Option<&[Context]>,
        limits: &Limits,
    ) -> Result<Self> {
        if parameters.context_bytes() > limits.decoded_bytes {
            return Err(Error::limit("JBIG2 refinement contexts exceed byte budget"));
        }
        let contexts = if let Some(retained) = retained {
            if retained.len() != parameters.context_count() {
                return Err(Error::syntax(
                    "incompatible JBIG2 retained refinement contexts",
                ));
            }
            retained.to_vec()
        } else {
            vec![Context::default(); parameters.context_count()]
        };
        Ok(Self {
            parameters,
            contexts,
        })
    }

    pub(super) fn context_bytes(&self) -> usize {
        self.contexts.len() * size_of::<Context>()
    }

    pub(super) fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        reference: &Bitmap,
        request: Request,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let Request {
            width,
            height,
            dx,
            dy,
            prediction,
        } = request;
        // Prediction inspects another 3x3 reference neighbourhood per pixel.
        work.pixels(u64::from(width) * u64::from(height), limits)?;
        work.pixels(u64::from(width) * u64::from(height), limits)?;
        let mut bitmap_limits = *limits;
        bitmap_limits.decoded_bytes = limits
            .decoded_bytes
            .checked_sub(self.context_bytes())
            .ok_or_else(|| Error::limit("JBIG2 refinement contexts exceed byte budget"))?;
        let mut bitmap = Bitmap::new(width, height, false, &bitmap_limits)?;
        let mut typical = 0;
        let typical_context = if self.parameters.kind == 0 { 0x10 } else { 8 };
        for y in 0..height {
            if prediction {
                typical ^= decoder.bit(&mut self.contexts[typical_context])?;
            }
            for x in 0..width {
                let rx = i64::from(x) - dx;
                let ry = i64::from(y) - dy;
                let implicit = if typical != 0 {
                    uniform(reference, rx, ry)
                } else {
                    None
                };
                let value = if let Some(value) = implicit {
                    value
                } else {
                    let index =
                        self.context(&bitmap, reference, i64::from(x), i64::from(y), rx, ry);
                    decoder.bit(&mut self.contexts[index])?
                };
                bitmap.set(x, y, value);
            }
        }
        Ok(bitmap)
    }

    fn context(
        &self,
        bitmap: &Bitmap,
        reference: &Bitmap,
        x: i64,
        y: i64,
        rx: i64,
        ry: i64,
    ) -> usize {
        let mut context = 0;
        let output: &[(i32, i32)] = if self.parameters.kind == 0 {
            &[(-1, 0), (1, -1), (0, -1), self.parameters.adaptive[0]]
        } else {
            &[(-1, 0), (1, -1), (0, -1), (-1, -1)]
        };
        for (bit, &(ox, oy)) in output.iter().enumerate() {
            context |= usize::from(bitmap.pixel(x + i64::from(ox), y + i64::from(oy))) << bit;
        }
        let positions: &[(i32, i32)] = if self.parameters.kind == 0 {
            &[
                (1, 1),
                (0, 1),
                (-1, 1),
                (1, 0),
                (0, 0),
                (-1, 0),
                (1, -1),
                (0, -1),
                self.parameters.adaptive[1],
            ]
        } else {
            &[(1, 1), (0, 1), (1, 0), (0, 0), (-1, 0), (0, -1)]
        };
        for (bit, &(ox, oy)) in positions.iter().enumerate() {
            context |= usize::from(reference.pixel(rx + i64::from(ox), ry + i64::from(oy)))
                << (bit + output.len());
        }
        context
    }
}

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) dx: i64,
    pub(super) dy: i64,
    pub(super) prediction: bool,
}

fn uniform(reference: &Bitmap, x: i64, y: i64) -> Option<u8> {
    let value = reference.pixel(x, y);
    for dy in -1..=1 {
        for dx in -1..=1 {
            if reference.pixel(x + dx, y + dy) != value {
                return None;
            }
        }
    }
    Some(value)
}
