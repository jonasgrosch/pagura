//! T.81 B.2.3 scan headers and the F.2 and G.1/G.2 entropy scans,
//! including successive approximation.
use super::entropy::{Bits, Huffman, Tables};
use super::frame::{Coding, Frame, Process};
use super::{Decoder, marker};
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostic, Diagnostics, Error, Result};

/// Work charged per 8×8 block in each scan: one unit per coefficient.
const BLOCK_WORK: usize = 64;
/// The most data units of an interleaved MCU (T.81 B.2.3).
const MCU_UNITS: usize = 10;
/// The largest successive-approximation bit position (T.81 Table B.3).
const MAX_APPROXIMATION: u8 = 13;

/// A component of a scan and its entropy coding table selectors.
#[derive(Debug, Clone, Copy)]
pub(super) struct Selected {
    /// Index into the frame's components.
    pub(super) component: usize,
    /// DC table selector.
    pub(super) dc: usize,
    /// AC table selector.
    pub(super) ac: usize,
}

pub(super) struct Scan {
    selected: Vec<Selected>,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) high: u8,
    pub(super) low: u8,
    pub(super) process: Process,
    /// Sample precision of the frame, 8 or 12 bits.
    pub(super) precision: u8,
}

impl Scan {
    pub(super) fn progressive(&self) -> bool {
        self.process == Process::Progressive
    }

    fn geometry(&self, frame: &Frame) -> Result<(usize, usize)> {
        let interleaved = self.selected.len() > 1;
        if interleaved {
            let units: usize = self
                .selected
                .iter()
                .filter_map(|s| frame.components.get(s.component))
                .map(|c| c.h * c.v)
                .sum();
            if units > MCU_UNITS {
                return Err(Error::syntax("too many JPEG data units in MCU"));
            }
            return Ok((frame.mcu_cols, frame.mcu_rows));
        }
        let c = self
            .selected
            .first()
            .and_then(|s| frame.components.get(s.component))
            .ok_or(Error::syntax("missing JPEG scan component"))?;
        let width = pagura_core::convert::u32_to_usize(frame.width);
        let height = pagura_core::convert::u32_to_usize(frame.height);
        Ok((
            (width * c.h).div_ceil(8 * frame.max_h),
            (height * c.v).div_ceil(8 * frame.max_v),
        ))
    }

    fn parse(mut data: Cursor<'_>, frame: &mut Frame) -> Result<Self> {
        let n = usize::from(data.u8()?);
        if n == 0 || n > frame.components.len() || data.remaining() != 2 * n + 3 {
            return Err(Error::syntax("invalid JPEG scan component count"));
        }
        let max_table = if frame.process == Process::Baseline {
            1
        } else {
            3
        };
        let mut selected: Vec<Selected> = Vec::with_capacity(n);
        for _ in 0..n {
            let [id, tables] = data.take_array()?;
            let component = frame
                .components
                .iter()
                .position(|c| c.id == id)
                .ok_or(Error::syntax("unknown JPEG scan component"))?;
            if selected.iter().any(|s| s.component == component)
                || tables >> 4 > max_table
                || tables & 15 > max_table
            {
                return Err(Error::syntax("invalid JPEG scan selector"));
            }
            selected.push(Selected {
                component,
                dc: usize::from(tables >> 4),
                ac: usize::from(tables & 15),
            });
        }
        let [start, end, approximation] = data.take_array()?;
        let (start, end, high, low) = (
            usize::from(start),
            usize::from(end),
            approximation >> 4,
            approximation & 15,
        );
        if start > end
            || end > 63
            || high > MAX_APPROXIMATION
            || low > MAX_APPROXIMATION
            || (high != 0 && high != low + 1)
        {
            return Err(Error::syntax(
                "invalid JPEG spectral selection or approximation",
            ));
        }
        if frame.progressive() {
            if (start == 0 && end != 0) || (start != 0 && n != 1) {
                return Err(Error::syntax("invalid progressive JPEG scan structure"));
            }
        } else if start != 0 || end != 63 || high != 0 || low != 0 {
            return Err(Error::syntax("invalid sequential JPEG scan structure"));
        }
        let (high_state, low_state) = (
            i8::try_from(high).map_err(|_| Error::syntax("invalid JPEG approximation"))?,
            i8::try_from(low).map_err(|_| Error::syntax("invalid JPEG approximation"))?,
        );
        for s in &selected {
            let approximation = &mut frame.components[s.component].approximation;
            if start > 0 && approximation[0] < 0 {
                return Err(Error::syntax("JPEG AC scan precedes DC scan"));
            }
            for state in &mut approximation[start..=end] {
                if (high == 0 && *state != -1) || (high != 0 && *state != high_state) {
                    return Err(Error::syntax("out-of-order JPEG coefficient scan"));
                }
                *state = low_state;
            }
        }
        Ok(Self {
            selected,
            start,
            end,
            high,
            low,
            process: frame.process,
            precision: frame.precision,
        })
    }
}

impl Decoder {
    /// Decodes the scan whose header is `payload` and whose entropy-coded
    /// data starts at `data`; returns the offset of the marker after it.
    pub(super) fn scan(
        &mut self,
        payload: Cursor<'_>,
        data: Cursor<'_>,
        diagnostics: &mut Diagnostics,
    ) -> Result<usize> {
        let frame = self
            .frame
            .as_mut()
            .ok_or(Error::syntax("JPEG scan before frame"))?;
        let scan = Scan::parse(payload, frame)?;
        let interleaved = scan.selected.len() > 1;
        let (cols, rows) = scan.geometry(frame)?;
        // Quantizers are latched at a component's first scan (T.81 B.2.4.1).
        let entropy = data.position();
        for s in &scan.selected {
            latch_quant(
                &mut frame.components[s.component],
                &self.quant,
                frame.process,
            )
            .map_err(|error| error.at(entropy))?;
        }
        let mut bits = Bits::new(data.clone());
        let mut arithmetic = match frame.coding {
            Coding::Huffman => None,
            Coding::Arithmetic => Some(super::arithmetic::Arithmetic::new(data.clone())?),
        };
        let mut predictors = [0i32; 4];
        let (mut eob, mut sequence) = (0u32, 0u8);
        for mcu in 0..cols * rows {
            if mcu > 0 && self.restart != 0 && mcu % self.restart == 0 {
                if eob != 0 {
                    return Err(bits.error("JPEG EOB run crosses restart"));
                }
                if let Some(coder) = &mut arithmetic {
                    coder.restart(sequence)?;
                } else {
                    bits.restart(sequence, diagnostics, &mut self.padding_reported)?;
                }
                sequence = (sequence + 1) % 8;
                predictors.fill(0);
            }
            for &s in &scan.selected {
                let c = &mut frame.components[s.component];
                let (horizontal, vertical) = if interleaved { (c.h, c.v) } else { (1, 1) };
                for y in 0..vertical {
                    for x in 0..horizontal {
                        self.budget.work(BLOCK_WORK)?;
                        let index = if interleaved {
                            (mcu / cols * c.v + y) * c.stride + mcu % cols * c.h + x
                        } else {
                            mcu / cols * c.stride + mcu % cols
                        };
                        let block = c
                            .blocks
                            .get_mut(index)
                            .ok_or_else(|| bits.error("JPEG block outside component"))?;
                        let predictor = &mut predictors[s.component];
                        if let Some(coder) = &mut arithmetic {
                            coder.block(block, &scan, &self.conditioning, s, predictor)?;
                        } else {
                            decode_block(
                                block,
                                &scan,
                                &self.huffman,
                                s,
                                predictor,
                                &mut eob,
                                &mut bits,
                            )?;
                        }
                    }
                }
            }
        }
        if eob != 0 {
            return Err(bits.error("JPEG EOB run exceeds scan"));
        }
        if let Some(coder) = arithmetic {
            return coder.finish();
        }
        bits.align(diagnostics, &mut self.padding_reported)?;
        let end = bits.offset();
        scan_end(data, end, &mut self.budget, diagnostics)
    }
}

fn latch_quant(
    component: &mut super::frame::Component,
    tables: &[Option<[u16; 64]>; super::frame::TABLES],
    process: Process,
) -> Result<()> {
    let quant = tables[component.table].ok_or(Error::syntax("missing JPEG quantization table"))?;
    if process == Process::Baseline && quant.iter().any(|&q| q > 255) {
        return Err(Error::syntax("baseline JPEG quantizer exceeds eight bits"));
    }
    if component.quant.is_some_and(|q| q != quant) {
        return Err(Error::syntax("JPEG quantizer changed between scans"));
    }
    component.quant = Some(quant);
    Ok(())
}

/// T.81 B.2.1 fixes the final segment's MCU count. Extra complete bytes can
/// be discarded only immediately before EOI; byte stuffing is still obeyed.
/// Returns the offset of the marker that ends the scan.
fn scan_end(
    data: Cursor<'_>,
    start: usize,
    budget: &mut crate::budget::Budget,
    diagnostics: &mut Diagnostics,
) -> Result<usize> {
    let mut bytes = data;
    bytes.seek(start)?;
    let excess = bytes.position();
    loop {
        budget.work(1)?;
        let at = bytes.position();
        let byte = bytes
            .u8()
            .map_err(|_| Error::syntax("missing JPEG scan terminator").at(at))?;
        if byte != marker::PREFIX {
            continue;
        }
        let fill = bytes
            .rest()
            .iter()
            .take_while(|&&b| b == marker::PREFIX)
            .count();
        budget.work(fill)?;
        bytes.skip(fill)?;
        let code = bytes
            .u8()
            .map_err(|_| Error::syntax("truncated JPEG scan marker").at(at))?;
        if code == marker::STUFFED {
            // Only FF 00 is a stuffed data byte; FF FF 00 is malformed.
            if fill != 0 {
                return Err(Error::syntax("invalid JPEG byte stuffing").at(at));
            }
            continue;
        }
        let offset = bytes.offset() - fill - 2;
        if at == excess {
            if matches!(code, marker::RST0..=marker::RST7)
                && bytes.rest().starts_with(&[marker::PREFIX, marker::EOI])
            {
                // No entropy is synthesized: the restart only ends a final
                // interval that has no MCUs left (T.81 B.2.1, B.2.4.4).
                diagnostics.repair(
                    Diagnostic::new(
                        crate::repairs::JPEG_TRAILING_RESTART,
                        "JPEG restart marker after final MCU ignored (T.81 B.2.1, B.2.4.4)",
                    )
                    .at(at),
                )?;
                return Ok(bytes.offset());
            }
            return Ok(offset);
        }
        if code != marker::EOI {
            return Err(Error::syntax("extra JPEG entropy data before another marker").at(excess));
        }
        diagnostics.repair(
            Diagnostic::new(
                crate::repairs::JPEG_ENTROPY_EXCESS,
                "extra JPEG entropy bytes before final EOI ignored (T.81 B.2.1)",
            )
            .at(excess),
        )?;
        return Ok(offset);
    }
}

/// The Huffman table a scan selects.
fn table<'t>(
    tables: &'t [Option<Huffman>; 4],
    index: usize,
    bits: &Bits<'_>,
) -> Result<&'t Huffman> {
    tables
        .get(index)
        .and_then(Option::as_ref)
        .ok_or_else(|| bits.error("missing JPEG Huffman table"))
}

fn decode_block(
    block: &mut [i32; 64],
    scan: &Scan,
    tables: &Tables,
    selected: Selected,
    predictor: &mut i32,
    eob: &mut u32,
    bits: &mut Bits<'_>,
) -> Result<()> {
    let (precision, progressive) = (scan.precision, scan.progressive());
    if scan.start == 0 {
        if scan.high == 0 {
            let size = table(&tables.dc, selected.dc, bits)?.symbol(bits)?;
            if size > precision + 3 {
                return Err(bits.error("invalid JPEG DC magnitude"));
            }
            *predictor = predictor
                .checked_add(bits.signed(size)?)
                .filter(|&v| {
                    let bound = 1 << (precision + 3);
                    (-bound..bound).contains(&v)
                })
                .ok_or_else(|| bits.error("JPEG DC predictor out of range"))?;
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
    let table = table(&tables.ac, selected.ac, bits)?;
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
            if size > precision + 2 {
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
