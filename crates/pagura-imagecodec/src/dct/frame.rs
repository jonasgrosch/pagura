//! T.81 B.2.2 frame headers, frame geometry and bounded coefficient storage.
use super::marker;
use crate::budget::{self, Budget};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Limits, Result};

/// The DCT process a start-of-frame marker selects (T.81 Table B.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Process {
    /// Baseline sequential: 8-bit samples, at most two tables of each kind.
    Baseline,
    /// Extended sequential.
    Extended,
    /// Progressive: spectral selection and successive approximation.
    Progressive,
}

/// The entropy coding a start-of-frame marker selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Coding {
    /// Huffman coding (T.81 Annex C, F.2.2, G.2).
    Huffman,
    /// Arithmetic coding (T.81 Annex D, F.2.4, G.2).
    Arithmetic,
}

impl Process {
    /// The process and coding of the DCT frame markers this decoder reads.
    pub(super) const fn of(code: u8) -> Option<(Self, Coding)> {
        match code {
            marker::SOF0 => Some((Self::Baseline, Coding::Huffman)),
            marker::SOF1 => Some((Self::Extended, Coding::Huffman)),
            marker::SOF2 => Some((Self::Progressive, Coding::Huffman)),
            marker::SOF9 => Some((Self::Extended, Coding::Arithmetic)),
            marker::SOF10 => Some((Self::Progressive, Coding::Arithmetic)),
            _ => None,
        }
    }
}

/// The largest sampling factor (T.81 B.2.2).
const MAX_SAMPLING: usize = 4;
/// The number of quantization tables (T.81 B.2.4.1).
pub(super) const TABLES: usize = 4;

pub(super) struct Component {
    pub(super) id: u8,
    pub(super) h: usize,
    pub(super) v: usize,
    pub(super) table: usize,
    pub(super) stride: usize,
    pub(super) rows: usize,
    pub(super) blocks: Vec<[i32; 64]>,
    pub(super) quant: Option<[u16; 64]>,
    pub(super) approximation: [i8; 64],
}

pub(super) struct Frame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) components: Vec<Component>,
    pub(super) max_h: usize,
    pub(super) max_v: usize,
    pub(super) mcu_cols: usize,
    pub(super) mcu_rows: usize,
    pub(super) process: Process,
    pub(super) coding: Coding,
    /// Sample precision in bits, 8 or 12.
    pub(super) precision: u8,
}

/// The fields of a frame header that describe the image.
pub(super) struct Header {
    pub(super) precision: u8,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) components: u8,
}

impl Header {
    /// Reads and checks the image fields of a frame header payload.
    pub(super) fn read(bytes: &mut Cursor<'_>, process: Process) -> Result<Self> {
        let precision = bytes.u8()?;
        if !matches!(precision, 8 | 12) || (process == Process::Baseline && precision != 8) {
            return Err(Error::unsupported(
                crate::features::JPEG_PRECISION,
                "JPEG process requires eight or twelve bit precision",
            ));
        }
        let height = u32::from(bytes.u16()?);
        let width = u32::from(bytes.u16()?);
        let components = bytes.u8()?;
        if width == 0 || components == 0 || bytes.remaining() != usize::from(components) * 3 {
            return Err(Error::syntax("invalid JPEG frame dimensions or components"));
        }
        if height == 0 {
            return Err(Error::unsupported(
                crate::features::JPEG_PROCESS,
                "JPEG height deferred to DNL is unsupported",
            ));
        }
        if !matches!(components, 1 | 3 | 4) {
            return Err(Error::unsupported(
                crate::features::JPEG_COMPONENTS,
                "JPEG component layout is unsupported",
            ));
        }
        Ok(Self {
            precision,
            width,
            height,
            components,
        })
    }
}

impl Frame {
    pub(super) fn parse(
        mut bytes: Cursor<'_>,
        process: Process,
        coding: Coding,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Self> {
        let header = Header::read(&mut bytes, process)?;
        let count = crate::sample_count(header.width, header.height, header.components, 8, limits)?;
        let mut components: Vec<Component> = Vec::with_capacity(usize::from(header.components));
        for _ in 0..header.components {
            let at = bytes.position();
            let [id, sampling, table] = bytes.take_array()?;
            let (h, v, table) = (
                usize::from(sampling >> 4),
                usize::from(sampling & 15),
                usize::from(table),
            );
            if !(1..=MAX_SAMPLING).contains(&h)
                || !(1..=MAX_SAMPLING).contains(&v)
                || table >= TABLES
                || components.iter().any(|p| p.id == id)
            {
                return Err(Error::syntax("invalid JPEG frame component").at(at));
            }
            components.push(Component {
                id,
                h,
                v,
                table,
                stride: 0,
                rows: 0,
                blocks: Vec::new(),
                quant: None,
                approximation: [-1; 64],
            });
        }
        let max_h = components.iter().map(|c| c.h).max().unwrap_or(1);
        let max_v = components.iter().map(|c| c.v).max().unwrap_or(1);
        let width = pagura_core::convert::u32_to_usize(header.width);
        let height = pagura_core::convert::u32_to_usize(header.height);
        let mcu_cols = width.div_ceil(8 * max_h);
        let mcu_rows = height.div_ceil(8 * max_v);
        let mut blocks = 0usize;
        for c in &mut components {
            c.stride = mcu_cols * c.h;
            c.rows = mcu_rows * c.v;
            blocks = c
                .stride
                .checked_mul(c.rows)
                .and_then(|n| blocks.checked_add(n))
                .ok_or(Error::limit("JPEG allocation overflow"))?;
        }
        // Everything the decode holds besides its output is charged before
        // any of it is allocated: the coefficients, a native-precision plane
        // per component and, for 12-bit samples, the expanded native samples
        // that the reduction to bytes reads.
        let coefficients = blocks
            .checked_mul(64)
            .ok_or(Error::limit("JPEG allocation overflow"))?;
        budget.memory::<i32>(coefficients)?;
        if header.precision == 8 {
            budget.memory::<u8>(coefficients)?;
        } else {
            budget.memory::<u16>(coefficients)?;
            budget.memory::<u16>(count)?;
        }
        budget.output_items::<u8>(count)?;
        for c in &mut components {
            c.blocks = budget::vector(c.stride * c.rows, [0; 64])?;
        }
        Ok(Self {
            width: header.width,
            height: header.height,
            components,
            max_h,
            max_v,
            mcu_cols,
            mcu_rows,
            process,
            coding,
            precision: header.precision,
        })
    }

    pub(super) fn progressive(&self) -> bool {
        self.process == Process::Progressive
    }
}
