//! T.81 frame geometry and bounded coefficient or sample storage.
use pagura_core::{Error, Limits, Result};

/// T.81 Table B.1 frame processes supported by this decoder.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Process {
    Baseline = 0xc0,
    Sequential = 0xc1,
    Progressive = 0xc2,
    Lossless = 0xc3,
    ArithmeticSequential = 0xc9,
    ArithmeticProgressive = 0xca,
}

impl TryFrom<u8> for Process {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0xc0 => Ok(Self::Baseline),
            0xc1 => Ok(Self::Sequential),
            0xc2 => Ok(Self::Progressive),
            0xc3 => Ok(Self::Lossless),
            0xc9 => Ok(Self::ArithmeticSequential),
            0xca => Ok(Self::ArithmeticProgressive),
            _ => Err(Error::syntax("invalid JPEG frame process")),
        }
    }
}

impl Process {
    pub(super) fn is_progressive(self) -> bool {
        matches!(self, Self::Progressive | Self::ArithmeticProgressive)
    }

    pub(super) fn is_arithmetic(self) -> bool {
        matches!(
            self,
            Self::ArithmeticSequential | Self::ArithmeticProgressive
        )
    }

    pub(super) fn is_baseline(self) -> bool {
        self == Self::Baseline
    }

    pub(super) fn is_lossless(self) -> bool {
        self == Self::Lossless
    }

    /// Width and height of a data unit: one sample or one DCT block (A.1.3).
    pub(super) fn unit(self) -> usize {
        if self.is_lossless() { 1 } else { 8 }
    }
}

/// A component's data units. A sequential scan completes each block before
/// the next (T.81 F.2), so its samples are reconstructed at once; progressive
/// scans refine coefficients until the frame ends (G.1.2).
pub(super) enum Storage {
    Coefficients(Vec<[i32; 64]>),
    Bytes(Vec<u8>),
    Words(Vec<u16>),
    /// Baseline samples decoded straight into a native plane.
    Native(Box<super::deferred::Plane>),
}

impl Storage {
    /// Heap bytes held for the component's samples or coefficients.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Coefficients(blocks) => blocks.capacity() * size_of::<[i32; 64]>(),
            Self::Bytes(samples) => samples.capacity(),
            Self::Words(samples) => samples.capacity() * 2,
            Self::Native(plane) => plane.bytes(),
        }
    }
}

impl Storage {
    fn new(units: usize, process: Process, precision: u8) -> Self {
        if process.is_lossless() {
            return Self::Words(vec![0; units]);
        }
        match (process.is_progressive(), precision > 8) {
            (true, _) => Self::Coefficients(vec![[0; 64]; units]),
            (false, false) => Self::Bytes(vec![0; units * 64]),
            (false, true) => Self::Words(vec![0; units * 64]),
        }
    }
}

pub(super) struct Component {
    pub(super) id: u8,
    pub(super) h: usize,
    pub(super) v: usize,
    pub(super) table: usize,
    pub(super) stride: usize,
    pub(super) rows: usize,
    pub(super) storage: Storage,
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
    pub(super) precision: u8,
}
impl Frame {
    /// Parses a frame header and allocates its components' storage.
    pub(super) fn parse(data: &[u8], process: u8, limits: &Limits) -> Result<Self> {
        let mut frame = Self::header(data, process, limits)?;
        for c in &mut frame.components {
            c.storage = Storage::new(c.stride * c.rows, frame.process, frame.precision);
        }
        Ok(frame)
    }

    /// Parses a frame header; its components hold no storage yet.
    pub(super) fn header(data: &[u8], process: u8, limits: &Limits) -> Result<Self> {
        let process = Process::try_from(process)?;
        if data.len() < 6 {
            return Err(Error::syntax("truncated JPEG frame"));
        }
        if process.is_lossless() && !(2..=16).contains(&data[0]) {
            return Err(Error::syntax(
                "lossless JPEG precision must be from two to sixteen bits",
            ));
        }
        if !process.is_lossless()
            && (!matches!(data[0], 8 | 12) || (process.is_baseline() && data[0] != 8))
        {
            return Err(Error::unsupported(
                crate::features::JPEG_PRECISION,
                "JPEG process requires eight or twelve bit precision",
            ));
        }
        let height = u32::from(u16::from_be_bytes([data[1], data[2]]));
        let width = u32::from(u16::from_be_bytes([data[3], data[4]]));
        let n = data[5];
        if width == 0 || n == 0 || data.len() != 6 + usize::from(n) * 3 {
            return Err(Error::syntax("invalid JPEG frame dimensions or components"));
        }
        if height == 0 {
            return Err(Error::unsupported(
                crate::features::JPEG_PROCESS,
                "JPEG height deferred to DNL is unsupported",
            ));
        }
        if !matches!(n, 1 | 3 | 4) {
            return Err(Error::unsupported(
                crate::features::JPEG_COMPONENTS,
                "JPEG component layout is unsupported",
            ));
        }
        super::super::parameters::budget(width, height, n, 8, limits)?;
        let mut components: Vec<Component> = Vec::with_capacity(usize::from(n));
        for c in data[6..].as_chunks::<3>().0 {
            let (h, v, table) = (
                usize::from(c[1] >> 4),
                usize::from(c[1] & 15),
                usize::from(c[2]),
            );
            if !(1..=4).contains(&h)
                || !(1..=4).contains(&v)
                || table > 3
                || (process.is_lossless() && table != 0)
                || components.iter().any(|p| p.id == c[0])
            {
                return Err(Error::syntax("invalid JPEG frame component"));
            }
            components.push(Component {
                id: c[0],
                h,
                v,
                table,
                stride: 0,
                rows: 0,
                storage: Storage::Coefficients(Vec::new()),
                quant: None,
                approximation: [-1; 64],
            });
        }
        let max_h = components
            .iter()
            .map(|c| c.h)
            .max()
            .ok_or(Error::syntax("missing JPEG components"))?;
        let max_v = components
            .iter()
            .map(|c| c.v)
            .max()
            .ok_or(Error::syntax("missing JPEG components"))?;
        let unit = process.unit();
        let mcu_cols = (width as usize).div_ceil(unit * max_h);
        let mcu_rows = (height as usize).div_ceil(unit * max_v);
        // Coefficients coexist with native planes and expanded samples. For
        // P=12 the latter coexist with the final byte buffer as well.
        // Sequential frames store native samples instead of coefficients;
        // they keep this bound.
        let words = data[0] > 8 || process.is_lossless();
        let mut memory =
            u64::from(width) * u64::from(height) * u64::from(n) * if words { 3 } else { 1 };
        for c in &mut components {
            c.stride = mcu_cols * c.h;
            c.rows = mcu_rows * c.v;
            let sample_bytes = if process.is_lossless() {
                2
            } else if words {
                6
            } else {
                5
            };
            memory = memory
                .checked_add(c.stride as u64 * c.rows as u64 * (unit * unit) as u64 * sample_bytes)
                .ok_or(Error::limit("JPEG allocation overflow"))?;
        }
        if memory > limits.decoded_bytes as u64 {
            return Err(Error::limit("JPEG coefficient and sample budget exceeded"));
        }
        Ok(Self {
            width,
            height,
            components,
            max_h,
            max_v,
            mcu_cols,
            mcu_rows,
            process,
            precision: data[0],
        })
    }
}
