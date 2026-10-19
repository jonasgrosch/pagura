//! T.81 F.2 and G.1/G.2 entropy scans, including successive approximation.
use super::{
    Decoder,
    entropy::{Bits, Huffman},
    frame::Storage,
    transform::{Cells, Idct, reconstruct},
};
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Result};

pub(super) struct Scan {
    selected: Vec<(usize, usize, usize)>,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) high: u8,
    pub(super) low: u8,
}
impl Scan {
    fn geometry(&self, frame: &super::Frame) -> Result<(usize, usize)> {
        let interleaved = self.selected.len() > 1;
        let geometry = if interleaved {
            (frame.mcu_cols, frame.mcu_rows)
        } else {
            let c = &frame.components[self.selected[0].0];
            (
                (frame.width as usize * c.h).div_ceil(8 * frame.max_h),
                (frame.height as usize * c.v).div_ceil(8 * frame.max_v),
            )
        };
        if interleaved
            && self
                .selected
                .iter()
                .map(|&(i, _, _)| frame.components[i].h * frame.components[i].v)
                .sum::<usize>()
                > 10
        {
            return Err(Error::syntax("too many JPEG data units in MCU"));
        }
        Ok(geometry)
    }

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
        let (cols, rows) = scan.geometry(frame)?;
        let mut bits = Bits::new(data, offset);
        let mut arithmetic = frame
            .arithmetic
            .then(|| {
                super::arithmetic::Arithmetic::new(
                    data,
                    offset,
                    limits.operations.saturating_sub(self.work),
                )
            })
            .transpose()?;
        let mut predictors = [0i32; 4];
        let (mut eob, mut sequence) = (0u32, 0u8);
        let idct = (!frame.progressive).then(Idct::new);
        let cells = (!frame.progressive && frame.cells < 8).then(|| Cells::new(frame.cells));
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
            for &(i, dc, ac) in &scan.selected {
                let c = &mut frame.components[i];
                // The tables cannot change within a scan (B.2.4.1). Validate
                // and latch in component order at its first MCU only.
                if mcu == 0 {
                    latch_quant(c, &self.quant, frame.baseline)
                        .map_err(|error| error.at(Position::Buffer(offset)))?;
                }
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
                        let mut sequential = [0; 64];
                        let block = match &mut c.storage {
                            Storage::Coefficients(blocks) => blocks
                                .get_mut(index)
                                .ok_or(bits.error("JPEG block outside component"))?,
                            _ => &mut sequential,
                        };
                        if let Some(coder) = &mut arithmetic {
                            coder.block(
                                block,
                                &scan,
                                frame.progressive,
                                &self.conditioning,
                                i,
                                (dc, ac),
                                &mut predictors[i],
                                frame.precision,
                            )?;
                        } else {
                            decode_block(
                                block,
                                &scan,
                                frame.progressive,
                                frame.precision,
                                &self.huffman,
                                (dc, ac),
                                &mut predictors[i],
                                &mut eob,
                                &mut bits,
                            )?;
                        }
                        if let Some(idct) = &idct {
                            store(c, idct, cells.as_ref(), &sequential, index)
                                .ok_or_else(|| bits.error("JPEG block outside component"))?;
                        }
                    }
                }
            }
        }
        if eob != 0 {
            return Err(bits.error("JPEG EOB run exceeds scan"));
        }
        self.finish(bits, arithmetic, data, limits, diagnostics)
    }

    /// Charges the scan's remaining entropy work and locates its end.
    fn finish(
        &mut self,
        mut bits: Bits<'_>,
        arithmetic: Option<super::arithmetic::Arithmetic<'_>>,
        data: &[u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<usize> {
        if let Some(coder) = arithmetic {
            let (offset, used) = coder.finish()?;
            self.work = self
                .work
                .checked_add(used)
                .filter(|&w| w <= limits.operations)
                .ok_or(Error::limit("JPEG arithmetic work budget exceeded"))?;
            return Ok(offset);
        }
        bits.align(diagnostics, &mut self.padding_reported)?;
        scan_end(data, bits.offset, &mut self.work, limits, diagnostics)
    }
}

/// Reconstructs a completed sequential block into its component's samples.
fn store(
    component: &mut super::frame::Component,
    idct: &Idct,
    cells: Option<&Cells>,
    block: &[i32; 64],
    index: usize,
) -> Option<()> {
    let quant = component.quant.as_ref()?;
    let stride = component.stride;
    match &mut component.storage {
        Storage::Bytes(plane) => match cells {
            Some(cells) => cells.reconstruct(idct, quant, block, plane, stride, index),
            None => reconstruct(idct, quant, block, plane, stride, index),
        },
        Storage::Words(plane) => reconstruct(idct, quant, block, plane, stride, index),
        Storage::Coefficients(_) => None,
    }
}

/// Quantizers are latched at a component's first scan (B.2.4.1).
fn latch_quant(
    component: &mut super::frame::Component,
    tables: &[Option<[u16; 64]>; 4],
    baseline: bool,
) -> Result<()> {
    let quant = tables[component.table].ok_or(Error::syntax("missing JPEG quantization table"))?;
    if baseline && quant.iter().any(|&q| q > 255) {
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
fn scan_end(
    data: &[u8],
    start: usize,
    work: &mut usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<usize> {
    let mut offset = start;
    loop {
        *work = work
            .checked_add(1)
            .filter(|&w| w <= limits.operations)
            .ok_or(Error::limit("JPEG scan work budget exceeded"))?;
        let byte = data.get(offset).ok_or_else(|| {
            Error::syntax("missing JPEG scan terminator").at(Position::Buffer(offset))
        })?;
        if *byte == 255 {
            let marker = offset;
            while data.get(offset) == Some(&255) {
                offset += 1;
                *work = work
                    .checked_add(1)
                    .filter(|&w| w <= limits.operations)
                    .ok_or(Error::limit("JPEG scan work budget exceeded"))?;
            }
            let code = data.get(offset).ok_or_else(|| {
                Error::syntax("truncated JPEG scan marker").at(Position::Buffer(marker))
            })?;
            if *code != 0 {
                if marker == start
                    && (0xd0..=0xd7).contains(code)
                    && data.get(offset + 1..offset + 3) == Some(&[255, 217])
                {
                    diagnostics.repair(
                        Diagnostic::new(
                            super::tags::TRAILING_RESTART,
                            "JPEG restart marker after final MCU ignored (T.81 B.2.1, B.2.4.4)",
                        )
                        .at(Position::Buffer(marker)),
                    )?;
                    return Ok(offset + 1);
                }
                if marker == start {
                    return Ok(marker);
                }
                if *code != 0xd9 {
                    return Err(
                        Error::syntax("extra JPEG entropy data before another marker")
                            .at(Position::Buffer(start)),
                    );
                }
                diagnostics.repair(
                    Diagnostic::new(
                        super::tags::ENTROPY_EXCESS,
                        "extra JPEG entropy bytes before final EOI ignored (T.81 B.2.1)",
                    )
                    .at(Position::Buffer(start)),
                )?;
                return Ok(marker);
            }
            // Only FF 00 is a stuffed data byte; FF FF 00 is malformed.
            if offset != marker + 1 {
                return Err(
                    Error::syntax("invalid JPEG byte stuffing").at(Position::Buffer(marker))
                );
            }
        }
        offset += 1;
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
    precision: u8,
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
            if size > precision + 3 {
                return Err(bits.error("invalid JPEG DC magnitude"));
            }
            *predictor = predictor
                .checked_add(bits.signed(size)?)
                .filter(|&v| {
                    let bound = 1 << (precision + 3);
                    (-bound..bound).contains(&v)
                })
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
