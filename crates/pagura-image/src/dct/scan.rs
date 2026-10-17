//! T.81 F.2 and G.1/G.2 entropy scans, including successive approximation.
use super::{
    Decoder,
    entropy::{Bits, Huffman},
};
use pagura_core::{Diagnostics, Error, Limits, Result};

struct Scan {
    selected: Vec<(usize, usize, usize)>,
    start: usize,
    end: usize,
    high: u8,
    low: u8,
}
impl Scan {
    fn parse(data: &[u8], frame: &mut super::Frame) -> Result<Self> {
        let n = usize::from(*data.first().ok_or(Error::syntax("missing JPEG scan"))?);
        if n == 0 || n > frame.components.len() || data.len() != 1 + 2 * n + 3 {
            return Err(Error::syntax("invalid JPEG scan component count"));
        }
        let mut selected = Vec::with_capacity(n);
        for pair in data[1..=2 * n].as_chunks::<2>().0 {
            let index = frame
                .components
                .iter()
                .position(|c| c.id == pair[0])
                .ok_or(Error::syntax("unknown JPEG scan component"))?;
            let max_table = if frame.baseline { 1 } else { 3 };
            if selected.iter().any(|&(i, _, _)| i == index)
                || pair[1] >> 4 > max_table
                || pair[1] & 15 > max_table
            {
                return Err(Error::syntax("invalid JPEG scan selector"));
            }
            selected.push((
                index,
                usize::from(pair[1] >> 4),
                4 + usize::from(pair[1] & 15),
            ));
        }
        let (start, end, high, low) = (
            usize::from(data[1 + 2 * n]),
            usize::from(data[2 + 2 * n]),
            data[3 + 2 * n] >> 4,
            data[3 + 2 * n] & 15,
        );
        if start > end || end > 63 || high > 13 || low > 13 || (high != 0 && high != low + 1) {
            return Err(Error::syntax(
                "invalid JPEG spectral selection or approximation",
            ));
        }
        if frame.progressive {
            if (start == 0 && end != 0) || (start != 0 && n != 1) {
                return Err(Error::syntax("invalid progressive JPEG scan structure"));
            }
        } else if start != 0 || end != 63 || high != 0 || low != 0 {
            return Err(Error::syntax("invalid sequential JPEG scan structure"));
        }
        for &(i, _, _) in &selected {
            if start > 0 && frame.components[i].approximation[0] < 0 {
                return Err(Error::syntax("JPEG AC scan precedes DC scan"));
            }
            for state in &mut frame.components[i].approximation[start..=end] {
                if (high == 0 && *state != -1)
                    || (high != 0
                        && *state
                            != i8::try_from(high)
                                .map_err(|_| Error::syntax("invalid JPEG approximation"))?)
                {
                    return Err(Error::syntax("out-of-order JPEG coefficient scan"));
                }
                *state =
                    i8::try_from(low).map_err(|_| Error::syntax("invalid JPEG approximation"))?;
            }
        }
        Ok(Self {
            selected,
            start,
            end,
            high,
            low,
        })
    }
}

impl Decoder {
    pub(super) fn scan(
        &mut self,
        payload: &[u8],
        data: &[u8],
        offset: usize,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<usize> {
        let frame = self
            .frame
            .as_mut()
            .ok_or(Error::syntax("JPEG scan before frame"))?;
        let scan = Scan::parse(payload, frame)?;
        let interleaved = scan.selected.len() > 1;
        let (cols, rows) = if interleaved {
            (frame.mcu_cols, frame.mcu_rows)
        } else {
            let c = &frame.components[scan.selected[0].0];
            (
                (frame.width as usize * c.h).div_ceil(8 * frame.max_h),
                (frame.height as usize * c.v).div_ceil(8 * frame.max_v),
            )
        };
        if interleaved
            && scan
                .selected
                .iter()
                .map(|&(i, _, _)| frame.components[i].h * frame.components[i].v)
                .sum::<usize>()
                > 10
        {
            return Err(Error::syntax("too many JPEG data units in MCU"));
        }
        let mut bits = Bits::new(data, offset);
        let mut predictors = [0i32; 4];
        let (mut eob, mut sequence) = (0u32, 0u8);
        for mcu in 0..cols * rows {
            if mcu > 0 && self.restart != 0 && mcu % self.restart == 0 {
                if eob != 0 {
                    return Err(bits.error("JPEG EOB run crosses restart"));
                }
                bits.restart(sequence, diagnostics, &mut self.padding_reported)?;
                sequence = (sequence + 1) % 8;
                predictors.fill(0);
            }
            for &(i, dc, ac) in &scan.selected {
                let c = &mut frame.components[i];
                let quant =
                    self.quant[c.table].ok_or(bits.error("missing JPEG quantization table"))?;
                if frame.baseline && quant.iter().any(|&q| q > 255) {
                    return Err(bits.error("baseline JPEG quantizer exceeds eight bits"));
                }
                if c.quant.is_some_and(|q| q != quant) {
                    return Err(bits.error("JPEG quantizer changed between scans"));
                }
                c.quant = Some(quant);
                let (horizontal, vertical) = if interleaved { (c.h, c.v) } else { (1, 1) };
                for y in 0..vertical {
                    for x in 0..horizontal {
                        self.work = self
                            .work
                            .checked_add(64)
                            .filter(|&w| w <= limits.operations)
                            .ok_or(Error::limit("JPEG scan work budget exceeded"))?;
                        let index = if interleaved {
                            (mcu / cols * c.v + y) * c.stride + mcu % cols * c.h + x
                        } else {
                            mcu / cols * c.stride + mcu % cols
                        };
                        let block = c
                            .blocks
                            .get_mut(index)
                            .ok_or(bits.error("JPEG block outside component"))?;
                        decode_block(
                            block,
                            &scan,
                            frame.progressive,
                            &self.huffman,
                            (dc, ac),
                            &mut predictors[i],
                            &mut eob,
                            &mut bits,
                        )?;
                    }
                }
            }
        }
        if eob != 0 {
            return Err(bits.error("JPEG EOB run exceeds scan"));
        }
        bits.align(diagnostics, &mut self.padding_reported)?;
        if data.get(bits.offset) != Some(&255) {
            return Err(bits.error("extra JPEG entropy data"));
        }
        Ok(bits.offset)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "entropy state belongs to the scan, tables, and individual component"
)]
fn decode_block(
    block: &mut [i32; 64],
    scan: &Scan,
    progressive: bool,
    tables: &[Option<Huffman>; 8],
    selectors: (usize, usize),
    predictor: &mut i32,
    eob: &mut u32,
    bits: &mut Bits<'_>,
) -> Result<()> {
    let missing = bits.error("missing JPEG Huffman table");
    let table = |index| {
        tables
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(|| missing.clone())
    };
    if scan.start == 0 {
        if scan.high == 0 {
            let size = table(selectors.0)?.symbol(bits)?;
            if size > 11 {
                return Err(bits.error("invalid JPEG DC magnitude"));
            }
            *predictor = predictor
                .checked_add(bits.signed(size)?)
                .filter(|v| (-2048..=2047).contains(v))
                .ok_or(bits.error("JPEG DC predictor out of range"))?;
            block[0] = *predictor << scan.low;
        } else {
            block[0] |= i32::try_from(bits.read(1)?)
                .map_err(|_| bits.error("invalid JPEG refinement"))?
                << scan.low;
        }
        if progressive {
            return Ok(());
        }
    }
    let table = table(selectors.1)?;
    if scan.high != 0 {
        return refine(block, scan, table, eob, bits);
    }
    if *eob > 0 {
        *eob -= 1;
        return Ok(());
    }
    let mut k = scan.start.max(1);
    while k <= scan.end {
        let symbol = table.symbol(bits)?;
        let (run, size) = (usize::from(symbol >> 4), symbol & 15);
        if size == 0 {
            if run == 15 {
                k += 16;
                if k > scan.end + 1 {
                    return Err(bits.error("JPEG zero run exceeds spectral band"));
                }
            } else if progressive {
                *eob = (1 << run)
                    + bits.read(u8::try_from(run).map_err(|_| bits.error("invalid JPEG run"))?)?
                    - 1;
                break;
            } else if run == 0 {
                break;
            } else {
                return Err(bits.error("invalid sequential JPEG AC symbol"));
            }
        } else {
            if size > 10 {
                return Err(bits.error("invalid JPEG AC magnitude"));
            }
            k += run;
            if k > scan.end {
                return Err(bits.error("JPEG AC run exceeds spectral band"));
            }
            block[k] = bits.signed(size)? << scan.low;
            k += 1;
        }
    }
    Ok(())
}

fn correction(value: &mut i32, bit: i32, bits: &mut Bits<'_>) -> Result<()> {
    if bits.read(1)? != 0 && value.abs() & bit == 0 {
        *value += if *value > 0 { bit } else { -bit };
    }
    Ok(())
}

fn refine(
    block: &mut [i32; 64],
    scan: &Scan,
    table: &Huffman,
    eob: &mut u32,
    bits: &mut Bits<'_>,
) -> Result<()> {
    let bit = 1 << scan.low;
    let mut k = scan.start;
    if *eob == 0 {
        while k <= scan.end {
            let symbol = table.symbol(bits)?;
            let (mut run, size) = (usize::from(symbol >> 4), symbol & 15);
            let new = match size {
                0 if run != 15 => {
                    *eob = (1 << run)
                        + bits.read(
                            u8::try_from(run).map_err(|_| bits.error("invalid JPEG EOB run"))?,
                        )?;
                    break;
                }
                0 => {
                    run = 16;
                    0
                }
                1 => {
                    if bits.read(1)? != 0 {
                        bit
                    } else {
                        -bit
                    }
                }
                _ => return Err(bits.error("invalid JPEG AC refinement symbol")),
            };
            while k <= scan.end {
                if block[k] != 0 {
                    correction(&mut block[k], bit, bits)?;
                } else if run == 0 {
                    break;
                } else {
                    run -= 1;
                }
                k += 1;
                if new == 0 && run == 0 {
                    break;
                }
            }
            if run != 0 || (new != 0 && k > scan.end) {
                return Err(bits.error("JPEG refinement run exceeds band"));
            }
            if new != 0 {
                block[k] = new;
                k += 1;
            }
        }
    }
    if *eob > 0 {
        for value in &mut block[k..=scan.end] {
            if *value != 0 {
                correction(value, bit, bits)?;
            }
        }
        *eob -= 1;
    }
    Ok(())
}
