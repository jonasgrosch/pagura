//! T.81 frame geometry and bounded coefficient or sample storage.
use pagura_core::{Error, Limits, Result};

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
    fn new(blocks: usize, progressive: bool, twelve_bit: bool) -> Self {
        match (progressive, twelve_bit) {
            (true, _) => Self::Coefficients(vec![[0; 64]; blocks]),
            (false, false) => Self::Bytes(vec![0; blocks * 64]),
            (false, true) => Self::Words(vec![0; blocks * 64]),
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
    pub(super) progressive: bool,
    pub(super) baseline: bool,
    pub(super) arithmetic: bool,
    pub(super) precision: u8,
}
impl Frame {
    /// Parses a frame header and allocates its components' storage.
    pub(super) fn parse(data: &[u8], process: u8, limits: &Limits) -> Result<Self> {
        let mut frame = Self::header(data, process, limits)?;
        for c in &mut frame.components {
            c.storage = Storage::new(c.stride * c.rows, frame.progressive, frame.precision == 12);
        }
        Ok(frame)
    }

    /// Parses a frame header; its components hold no storage yet.
    pub(super) fn header(data: &[u8], process: u8, limits: &Limits) -> Result<Self> {
        if data.len() < 6 {
            return Err(Error::syntax("truncated JPEG frame"));
        }
        if !matches!(data[0], 8 | 12) || (process == 0xc0 && data[0] != 8) {
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
        let mcu_cols = (width as usize).div_ceil(8 * max_h);
        let mcu_rows = (height as usize).div_ceil(8 * max_v);
        // Progressive coefficients (four bytes) coexist with the native
        // planes reconstructed from them and with the expanded samples.
        // Sequential frames reconstruct native samples into their planes
        // during the scans, so only planes and expanded samples coexist. For
        // P=12 planes and samples are words, and the samples coexist with
        // the final byte buffer as well. Colour conversion adds rows of at
        // most 28 bytes per pixel of width.
        let progressive = matches!(process, 0xc2 | 0xca);
        let twelve_bit = data[0] == 12;
        let plane_bytes = u64::from(twelve_bit) + 1;
        let per_sample = if progressive {
            4 + plane_bytes
        } else {
            plane_bytes
        };
        let mut memory =
            u64::from(width) * u64::from(height) * u64::from(n) * if twelve_bit { 3 } else { 1 }
                + u64::from(width) * 32;
        for c in &mut components {
            c.stride = mcu_cols * c.h;
            c.rows = mcu_rows * c.v;
            memory = memory
                .checked_add((c.stride * c.rows) as u64 * 64 * per_sample)
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
            progressive,
            arithmetic: matches!(process, 0xc9 | 0xca),
            precision: data[0],
            baseline: process == 0xc0,
        })
    }
}
