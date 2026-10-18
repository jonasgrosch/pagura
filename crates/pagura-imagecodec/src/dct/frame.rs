//! T.81 frame geometry and bounded coefficient storage.
use pagura_core::{Error, Limits, Result};

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
    pub(super) progressive: bool,
    pub(super) baseline: bool,
    pub(super) arithmetic: bool,
    pub(super) precision: u8,
}
impl Frame {
    pub(super) fn parse(data: &[u8], process: u8, limits: &Limits) -> Result<Self> {
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
                blocks: Vec::new(),
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
        // Coefficients coexist with native planes and expanded samples. For
        // P=12 the latter coexist with the final byte buffer as well.
        let twelve_bit = data[0] == 12;
        let mut memory =
            u64::from(width) * u64::from(height) * u64::from(n) * if twelve_bit { 3 } else { 1 };
        for c in &mut components {
            c.stride = mcu_cols * c.h;
            c.rows = mcu_rows * c.v;
            memory = memory
                .checked_add((c.stride * c.rows) as u64 * 64 * if twelve_bit { 6 } else { 5 })
                .ok_or(Error::limit("JPEG allocation overflow"))?;
        }
        if memory > limits.decoded_bytes as u64 {
            return Err(Error::limit("JPEG coefficient and sample budget exceeded"));
        }
        for c in &mut components {
            c.blocks = vec![[0; 64]; c.stride * c.rows];
        }
        Ok(Self {
            width,
            height,
            components,
            max_h,
            max_v,
            mcu_cols,
            mcu_rows,
            progressive: matches!(process, 0xc2 | 0xca),
            arithmetic: matches!(process, 0xc9 | 0xca),
            precision: data[0],
            baseline: process == 0xc0,
        })
    }
}
