//! T.81 F.2 and G.1/G.2 entropy scans, including successive approximation.
use super::{
    Decoder,
    entropy::{Bits, Huffman},
    frame::Storage,
    transform::{Idct, reconstruct},
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
        let mut bits = entropy_bits(frame, data, offset, limits, self.work);
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
        // Sequential blocks share one buffer, cleared after each block up to
        // the coefficients that block wrote.
        let mut sequential = [0; 64];
        for mcu in 0..cols * rows {
            let (row, column) = (mcu / cols, mcu % cols);
            if mcu > 0 && self.restart != 0 && mcu % self.restart == 0 {
                let (entropy, reported) =
                    ((&mut bits, arithmetic.as_mut()), &mut self.padding_reported);
                restart(entropy, (eob, &mut sequence), (diagnostics, reported))?;
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
                        self.work = block_work(self.work, limits)?;
                        let (block_row, block_column) = if interleaved {
                            (row * c.v + y, column * c.h + x)
                        } else {
                            (row, column)
                        };
                        let index = block_row * c.stride + block_column;
                        if let Storage::Native(plane) = &mut c.storage {
                            let tables = (&self.huffman, dc, ac);
                            let block = ((block_row, block_column), &mut predictors[i]);
                            native(plane, c.quant.as_ref(), tables, block, &mut bits)?;
                            continue;
                        }
                        let block = match &mut c.storage {
                            Storage::Coefficients(blocks) => blocks
                                .get_mut(index)
                                .ok_or_else(|| bits.error("JPEG block outside component"))?,
                            _ => &mut sequential,
                        };
                        let written = if let Some(coder) = &mut arithmetic {
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
                            64
                        } else {
                            decode_block(
                                block,
                                &scan,
                                frame.progressive,
                                frame.precision,
                                (
                                    self.huffman.get(dc).and_then(Option::as_ref),
                                    self.huffman.get(ac).and_then(Option::as_ref),
                                ),
                                &mut predictors[i],
                                &mut eob,
                                &mut bits,
                            )?
                        };
                        if let Some(idct) = &idct {
                            store(c, idct, &sequential[..written], index)
                                .ok_or_else(|| bits.error("JPEG block outside component"))?;
                            sequential[..written].fill(0);
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
        self.work = self
            .work
            .checked_add(bits.stream_work())
            .filter(|&w| w <= limits.operations)
            .ok_or(Error::limit("JPEG entropy work exceeded"))?;
        bits.align(diagnostics, &mut self.padding_reported)?;
        scan_end(data, bits.offset(), &mut self.work, limits, diagnostics)
    }
}

/// Resets the entropy decoder at the next restart marker, which must follow
/// a complete EOB run and carry the expected sequence number (T.81 B.2.1).
fn restart(
    (bits, arithmetic): (
        &mut Bits<'_>,
        Option<&mut super::arithmetic::Arithmetic<'_>>,
    ),
    (eob, sequence): (u32, &mut u8),
    (diagnostics, reported): (&mut Diagnostics, &mut bool),
) -> Result<()> {
    if eob != 0 {
        return Err(bits.error("JPEG EOB run crosses restart"));
    }
    if let Some(coder) = arithmetic {
        coder.restart(*sequence)?;
    } else {
        bits.restart(*sequence, diagnostics, reported)?;
    }
    *sequence = (*sequence + 1) % 8;
    Ok(())
}

/// Charges one data unit's 64 coefficients against the work budget.
fn block_work(work: usize, limits: &Limits) -> Result<usize> {
    work.checked_add(64)
        .filter(|&w| w <= limits.operations)
        .ok_or(Error::limit("JPEG scan work budget exceeded"))
}

/// An entropy reader for a scan. Native planes copy each segment without
/// its stuffing, within the storage and work left by the frame.
fn entropy_bits<'a>(
    frame: &super::Frame,
    data: &'a [u8],
    offset: usize,
    limits: &Limits,
    work: usize,
) -> Bits<'a> {
    let mut bits = Bits::new(data, offset);
    if frame
        .components
        .iter()
        .any(|c| matches!(c.storage, Storage::Native(_)))
    {
        let held: usize = frame.components.iter().map(|c| c.storage.bytes()).sum();
        bits.limit_stream(
            limits.decoded_bytes.saturating_sub(held),
            limits.operations.saturating_sub(work),
        );
    }
    bits
}

/// Decodes the block at (row, column) of a component held in a native plane.
fn native(
    plane: &mut super::deferred::Plane,
    quant: Option<&[u16; 64]>,
    (tables, dc, ac): (&[Option<Huffman>; 8], usize, usize),
    (block, predictor): ((usize, usize), &mut i32),
    bits: &mut Bits<'_>,
) -> Result<()> {
    let (Some(dc), Some(ac)) = (
        tables.get(dc).and_then(Option::as_ref),
        tables.get(ac).and_then(Option::as_ref),
    ) else {
        return Err(bits.error("missing JPEG Huffman table"));
    };
    let quant = quant.ok_or_else(|| bits.error("missing JPEG quantizer"))?;
    plane.decode(block, dc, ac, bits, predictor, quant)
}

/// Reconstructs a completed sequential block into its component's samples.
fn store(
    component: &mut super::frame::Component,
    idct: &Idct,
    block: &[i32],
    index: usize,
) -> Option<()> {
    let quant = component.quant.as_ref()?;
    let stride = component.stride;
    match &mut component.storage {
        Storage::Bytes(plane) => reconstruct(idct, quant, block, plane, stride, index),
        Storage::Words(plane) => reconstruct(idct, quant, block, plane, stride, index),
        Storage::Coefficients(_) | Storage::Native(_) => None,
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
    (dc, ac): (Option<&Huffman>, Option<&Huffman>),
    predictor: &mut i32,
    eob: &mut u32,
    bits: &mut Bits<'_>,
) -> Result<usize> {
    // One past the last coefficient written; later ones are unchanged.
    let mut written = usize::from(scan.start == 0);
    // A missing table is reported at the data unit's first bit, even after
    // its DC difference has been decoded.
    let unit = (dc.is_none() || ac.is_none()).then(|| bits.offset());
    let missing = || {
        Error::syntax("missing JPEG Huffman table").at(Position::Buffer(unit.unwrap_or_default()))
    };
    if scan.start == 0 {
        if scan.high == 0 {
            let table = dc.ok_or_else(missing)?;
            let code = table.code(bits)?;
            let size = code.symbol;
            if size > precision + 3 {
                bits.skip(code);
                return Err(bits.error("invalid JPEG DC magnitude"));
            }
            // Both the predictor and a difference are below 2^15 in magnitude.
            let value = *predictor + bits.magnitude(code, size)?;
            let bound = 1 << (precision + 3);
            if !(-bound..bound).contains(&value) {
                return Err(bits.error("JPEG DC predictor out of range"));
            }
            *predictor = value;
            block[0] = *predictor << scan.low;
        } else {
            block[0] |= i32::try_from(bits.read(1)?)
                .map_err(|_| bits.error("invalid JPEG refinement"))?
                << scan.low;
        }
        if progressive {
            return Ok(written);
        }
    }
    let table = ac.ok_or_else(missing)?;
    if scan.high != 0 {
        refine(block, scan, table, eob, bits)?;
        return Ok(scan.end + 1);
    }
    if *eob > 0 {
        *eob -= 1;
        return Ok(written);
    }
    // The spectral band; its end is validated at most 63 (T.81 B.2.3).
    let band = &mut block[..=scan.end];
    let mut k = scan.start.max(1);
    while k < band.len() {
        let code = table.code(bits)?;
        let (run, size) = (usize::from(code.symbol >> 4), code.symbol & 15);
        if size == 0 {
            bits.skip(code);
            if run == 15 {
                k += 16;
                if k > band.len() {
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
                bits.skip(code);
                return Err(bits.error("invalid JPEG AC magnitude"));
            }
            k += run;
            let Some(coefficient) = band.get_mut(k) else {
                bits.skip(code);
                return Err(bits.error("JPEG AC run exceeds spectral band"));
            };
            *coefficient = bits.magnitude(code, size)? << scan.low;
            k += 1;
            written = k;
        }
    }
    Ok(written)
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
