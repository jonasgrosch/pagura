//! T.81 F.2 and G.1/G.2 entropy scans, including successive approximation.
use super::{
    Decoder,
    entropy::{Bits, Huffman},
    frame::Storage,
    transform::{Cells, Idct, Sparse, reconstruct},
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
        // Sequential blocks share one buffer, cleared after each block.
        let mut sequential = [0; 64];
        let cells = (!frame.progressive && frame.cells < 8).then(|| Cells::new(frame.cells));
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
                        // The positions of the block's nonzero coefficients,
                        // when known.
                        let nonzero = if let Some(coder) = &mut arithmetic {
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
                            None
                        } else {
                            huffman_block(
                                block,
                                &scan,
                                (frame.progressive, frame.precision),
                                (
                                    self.huffman.get(dc).and_then(Option::as_ref),
                                    self.huffman.get(ac).and_then(Option::as_ref),
                                ),
                                (&mut predictors[i], &mut eob),
                                &mut bits,
                            )?
                        };
                        if let Some(idct) = &idct {
                            store(c, idct, cells.as_ref(), (&sequential, nonzero), index)
                                .ok_or_else(|| bits.error("JPEG block outside component"))?;
                            sequential = [0; 64];
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

/// Reconstructs a completed sequential block into its component's samples,
/// given the positions of its nonzero coefficients when they are known.
fn store(
    component: &mut super::frame::Component,
    idct: &Idct,
    cells: Option<&Cells>,
    (block, nonzero): (&[i32; 64], Option<u64>),
    index: usize,
) -> Option<()> {
    let block = nonzero.map_or_else(
        || Sparse::new(block),
        |nonzero| Sparse::known(block, nonzero),
    );
    let quant = component.quant.as_ref()?;
    let stride = component.stride;
    match &mut component.storage {
        Storage::Bytes(plane) => match cells {
            Some(cells) => cells.reconstruct(idct, quant, block, plane, stride, index),
            None => reconstruct(idct, quant, block, plane, stride, index),
        },
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

/// Decodes one Huffman-coded block, by the register decoder where it
/// applies; returns the positions of the nonzero coefficients when that
/// decoder decoded the block.
fn huffman_block(
    block: &mut [i32; 64],
    scan: &Scan,
    (progressive, precision): (bool, u8),
    tables: (Option<&Huffman>, Option<&Huffman>),
    (predictor, eob): (&mut i32, &mut u32),
    bits: &mut Bits<'_>,
) -> Result<Option<u64>> {
    if let (Some(dc), Some(ac)) = tables
        && !progressive
        && let Some(nonzero) = bits.sequential((dc, ac), precision, predictor, block)
    {
        return Ok(Some(nonzero));
    }
    decode_block(
        block,
        scan,
        progressive,
        precision,
        tables,
        predictor,
        eob,
        bits,
    )?;
    Ok(None)
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
) -> Result<()> {
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
            return Ok(());
        }
    }
    let table = ac.ok_or_else(missing)?;
    if scan.high != 0 {
        return refine(block, scan, table, eob, bits);
    }
    if *eob > 0 {
        *eob -= 1;
        return Ok(());
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

#[cfg(test)]
mod tests {
    use super::{Bits, Scan, Sparse, decode_block};

    /// A xorshift generator.
    struct Random(u64);
    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// A DHT payload for one table: random code counts that leave the
    /// all-ones code free at every length (T.81 C.2), over `symbols`.
    fn table(random: &mut Random, class: u8, symbols: &[u8]) -> Vec<u8> {
        let mut counts = [0u8; 16];
        let (mut free, mut left) = (2u64, symbols.len());
        for count in &mut counts {
            let most = u64::try_from(left).unwrap().min(free - 1).min(24);
            let chosen = if most == 0 { 0 } else { random.below(most + 1) };
            *count = u8::try_from(chosen).unwrap();
            left -= usize::from(*count);
            free = (free - chosen) * 2;
        }
        let total = symbols.len() - left;
        let mut payload = vec![class];
        payload.extend_from_slice(&counts);
        payload.extend_from_slice(&symbols[..total]);
        payload
    }

    /// Distinct symbols: DC sizes, some past eight-bit precision, or AC run
    /// and size pairs led by EOB, with invalid sizes and runs among them.
    fn symbols(random: &mut Random, ac: bool) -> Vec<u8> {
        let mut pool: Vec<u8> = if ac {
            (0..=255).filter(|&s| s & 15 <= 12).collect()
        } else {
            (0..=13).collect()
        };
        for i in (1..pool.len()).rev() {
            pool.swap(i, usize::try_from(random.below(i as u64 + 1)).unwrap());
        }
        if ac && let Some(eob) = pool.iter().position(|&s| s == 0) {
            pool.swap(eob, usize::try_from(random.below(3)).unwrap());
        }
        pool.truncate(usize::try_from(3 + random.below(if ac { 160 } else { 11 })).unwrap());
        pool
    }

    /// Each symbol's canonical code and length in a DHT payload (T.81 C.2).
    fn codes(payload: &[u8]) -> Vec<(u8, u32, u32)> {
        let (mut code, mut index, mut out) = (0u32, 0usize, Vec::new());
        for (length, &count) in (1..=16).zip(&payload[1..17]) {
            for _ in 0..count {
                out.push((payload[17 + index], code, length));
                (code, index) = (code + 1, index + 1);
            }
            code *= 2;
        }
        out
    }

    /// Bits written most significant first, with FF 00 stuffing.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        byte: u32,
        bits: u32,
    }
    impl Writer {
        fn put(&mut self, value: u32, width: u32) {
            for bit in (0..width).rev() {
                self.byte = self.byte * 2 + (value >> bit & 1);
                self.bits += 1;
                if self.bits == 8 {
                    let byte = u8::try_from(self.byte).unwrap();
                    self.bytes.push(byte);
                    if byte == 255 {
                        self.bytes.push(0);
                    }
                    (self.byte, self.bits) = (0, 0);
                }
            }
        }
    }

    /// Entropy-coded blocks of random valid symbols, with a random
    /// magnitude for each, then damage: flipped bits, an inserted marker,
    /// a truncation, or none; the data ends in EOI or another marker or
    /// stops.
    fn data(random: &mut Random, dc: &[(u8, u32, u32)], ac: &[(u8, u32, u32)]) -> Option<Vec<u8>> {
        let mut writer = Writer::default();
        let pick = |random: &mut Random, from: &[&(u8, u32, u32)]| {
            *from[usize::try_from(random.below(from.len() as u64)).unwrap()]
        };
        let sizes: Vec<_> = dc.iter().filter(|c| c.0 <= 6).collect();
        if sizes.is_empty() {
            return None;
        }
        for _ in 0..50 + random.below(250) {
            let (size, code, length) = pick(random, &sizes);
            writer.put(code, length);
            writer.put(
                u32::try_from(random.below(1 << size)).unwrap(),
                u32::from(size),
            );
            let mut k = 1;
            while k < 64 {
                if random.below(5) == 0
                    && let Some(&(_, code, length)) = ac.iter().find(|c| c.0 == 0)
                {
                    writer.put(code, length);
                    break;
                }
                let fitting: Vec<_> = ac
                    .iter()
                    .filter(|c| {
                        let (run, size) = (usize::from(c.0 >> 4), c.0 & 15);
                        (1..=10).contains(&size) && k + run < 64 || c.0 == 0xf0 && k + 16 <= 63
                    })
                    .collect();
                if fitting.is_empty() {
                    break;
                }
                let (symbol, code, length) = pick(random, &fitting);
                writer.put(code, length);
                let (run, size) = (usize::from(symbol >> 4), symbol & 15);
                writer.put(
                    u32::try_from(random.below(1 << size)).unwrap(),
                    u32::from(size),
                );
                k += if size == 0 { 16 } else { run + 1 };
            }
        }
        writer.put(255, 7);
        let mut data = writer.bytes;
        let at = usize::try_from(random.below(data.len() as u64)).unwrap();
        match random.below(4) {
            0 => {
                for _ in 0..=random.below(3) {
                    let at = usize::try_from(random.below(data.len() as u64)).unwrap();
                    data[at] ^= 1 << random.below(8);
                }
            }
            1 => data.splice(at..at, [255, 0xd0]).for_each(drop),
            2 => data.truncate(at),
            _ => {}
        }
        match random.below(3) {
            0 => data.extend_from_slice(&[255, 217]),
            1 => data.extend_from_slice(&[255, 0xc4]),
            _ => {}
        }
        Some(data)
    }

    /// The next bits a reader yields, or its error.
    fn following(bits: &Bits<'_>) -> Vec<Result<u32, (&'static str, pagura_core::Position)>> {
        let mut bits = bits.clone();
        (0..4)
            .map(|_| bits.read(13).map_err(|e| (e.message, e.position)))
            .collect()
    }

    #[test]
    fn register_blocks_match_the_reference_decoder_or_leave_the_reader_unchanged() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let (mut decoded, mut declined, mut failed) = (0, 0, 0);
        for round in 0..400 {
            let precision = if round % 4 == 3 { 12 } else { 8 };
            let (sizes, pairs) = (symbols(&mut random, false), symbols(&mut random, true));
            let mut payload = table(&mut random, 0x00, &sizes);
            let sizes_used = payload.len() - 17;
            payload.extend(table(&mut random, 0x10, &pairs));
            let mut books = std::array::from_fn(|_| None);
            if crate::dct::entropy::tables(&payload, &mut books, false).is_err() {
                continue;
            }
            let (Some(dc), Some(ac)) = (books[0].as_ref(), books[4].as_ref()) else {
                continue;
            };
            let scan = Scan {
                selected: vec![(0, 0, 4)],
                start: 0,
                end: 63,
                high: 0,
                low: 0,
            };
            let Some(data) = data(
                &mut random,
                &codes(&payload),
                &codes(&payload[17 + sizes_used..]),
            ) else {
                continue;
            };
            let mut bits = Bits::new(&data, 0);
            let mut predictor = 0;
            loop {
                let (mut reference, mut expected, mut eob) = (bits.clone(), [0; 64], 0);
                let mut reference_predictor = predictor;
                let outcome = decode_block(
                    &mut expected,
                    &scan,
                    false,
                    precision,
                    (Some(dc), Some(ac)),
                    &mut reference_predictor,
                    &mut eob,
                    &mut reference,
                );
                let (mut fast, mut block, mut fast_predictor) = (bits.clone(), [0; 64], predictor);
                if let Some(nonzero) =
                    fast.sequential((dc, ac), precision, &mut fast_predictor, &mut block)
                {
                    assert!(outcome.is_ok(), "round {round}");
                    assert_eq!(block, expected, "round {round}");
                    assert_eq!(nonzero, Sparse::new(&block).nonzero());
                    assert_eq!(fast_predictor, reference_predictor);
                    assert_eq!(fast.offset(), reference.offset());
                    assert_eq!(following(&fast), following(&reference), "round {round}");
                    decoded += 1;
                } else {
                    assert_eq!((block, fast_predictor), ([0; 64], predictor));
                    assert_eq!(fast.offset(), bits.offset());
                    assert_eq!(following(&fast), following(&bits));
                    declined += 1;
                }
                if outcome.is_err() {
                    failed += 1;
                    break;
                }
                (bits, predictor) = (reference, reference_predictor);
            }
        }
        assert!(
            decoded > 20_000 && declined > 400 && failed > 300,
            "{decoded} {declined} {failed}"
        );
    }
}
