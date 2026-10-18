//! Huffman text and aggregation, T.88 (02/2000), 6.4 and 7.4.3.
use crate::jbig2::{
    Bitmap,
    arithmetic::Decoder,
    budget::Work,
    huffman::{Bits, Table},
    reader::Reader,
    refinement::{Model, Parameters, Request},
    text::Config,
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

pub(crate) fn select(
    selection: u16,
    standards: &[usize],
    custom: &mut std::slice::Iter<'_, Rc<Table>>,
    oob: bool,
) -> Result<Rc<Table>> {
    let table = if usize::from(selection) < standards.len() {
        Rc::new(Table::standard(standards[usize::from(selection)])?)
    } else if selection == 3 {
        custom
            .next()
            .cloned()
            .ok_or_else(|| Error::syntax("missing JBIG2 user Huffman table"))?
    } else {
        return Err(Error::syntax("reserved JBIG2 Huffman table selection"));
    };
    if table.has_oob() != oob {
        return Err(Error::syntax(
            "incorrect JBIG2 Huffman table OOB capability",
        ));
    }
    Ok(table)
}

pub(crate) struct Procedure {
    tables: Vec<Rc<Table>>,
    pub(crate) model: Option<Model>,
}
impl Procedure {
    pub(crate) fn aggregation(model: Model) -> Result<Self> {
        Ok(Self {
            tables: [6, 8, 11, 15, 15, 15, 15, 1]
                .into_iter()
                .map(|n| Table::standard(n).map(Rc::new))
                .collect::<Result<_>>()?,
            model: Some(model),
        })
    }
    fn read(
        reader: &mut Reader<'_>,
        flags: u16,
        custom: &[Rc<Table>],
        limits: &Limits,
    ) -> Result<Self> {
        let selections = reader.u16()?;
        if selections & 0x8000 != 0
            || flags & 2 == 0 && (selections & 0x7fc0 != 0 || flags & 0x8000 != 0)
        {
            return Err(Error::syntax("invalid JBIG2 text Huffman flags"));
        }
        let mut custom = custom.iter();
        let tables = vec![
            select(selections & 3, &[6, 7], &mut custom, false)?,
            select((selections >> 2) & 3, &[8, 9, 10], &mut custom, true)?,
            select((selections >> 4) & 3, &[11, 12, 13], &mut custom, false)?,
            select((selections >> 6) & 3, &[14, 15], &mut custom, false)?,
            select((selections >> 8) & 3, &[14, 15], &mut custom, false)?,
            select((selections >> 10) & 3, &[14, 15], &mut custom, false)?,
            select((selections >> 12) & 3, &[14, 15], &mut custom, false)?,
            select(
                if selections & 0x4000 != 0 { 3 } else { 0 },
                &[1],
                &mut custom,
                false,
            )?,
        ];
        if custom.next().is_some() {
            return Err(Error::syntax("unused JBIG2 user Huffman table"));
        }
        let model = if flags & 2 != 0 {
            Some(Model::new(
                Parameters::read(reader, u8::from(flags & 0x8000 != 0))?,
                None,
                limits,
            )?)
        } else {
            None
        };
        Ok(Self { tables, model })
    }
    pub(crate) fn bytes(&self) -> usize {
        self.tables.iter().map(|t| t.bytes()).sum::<usize>()
            + self.model.as_ref().map_or(0, Model::context_bytes)
    }
    fn number(&self, n: usize, bits: &mut Bits<'_>) -> Result<i64> {
        self.tables[n].required(bits)
    }
    fn refined(
        &mut self,
        bits: &mut Bits<'_>,
        symbol: &Bitmap,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Option<Bitmap>> {
        if self.model.is_none() || bits.read(1)? == 0 {
            return Ok(None);
        }
        let dw = self.number(3, bits)?;
        let dh = self.number(4, bits)?;
        let request = Request {
            width: dimension(symbol.width, dw)?,
            height: dimension(symbol.height, dh)?,
            dx: (dw >> 1) + self.number(5, bits)?,
            dy: (dh >> 1) + self.number(6, bits)?,
            prediction: false,
        };
        self.bitmap(bits, symbol, request, limits, work).map(Some)
    }
    fn bitmap(
        &mut self,
        bits: &mut Bits<'_>,
        symbol: &Bitmap,
        request: Request,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let count = usize::try_from(self.number(7, bits)?)
            .map_err(|_| Error::syntax("negative JBIG2 refinement data size"))?;
        bits.align();
        let base = match bits.position() {
            Position::Buffer(n) => n,
            _ => 0,
        };
        let data = bits.take(count)?;
        self.model
            .as_mut()
            .ok_or_else(|| Error::syntax("missing JBIG2 refinement model"))?
            .decode(
                &mut Decoder::new(data, base)?,
                symbol,
                request,
                limits,
                work,
            )
    }
    pub(crate) fn single(
        &mut self,
        bits: &mut Bits<'_>,
        symbols: &[Rc<Bitmap>],
        size: (u32, u32),
        id_bits: u8,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let id = bits.read(id_bits)? as usize;
        let symbol = symbols
            .get(id)
            .ok_or_else(|| Error::syntax("JBIG2 aggregation identifier out of range"))?;
        let request = Request {
            width: size.0,
            height: size.1,
            dx: self.number(5, bits)?,
            dy: self.number(6, bits)?,
            prediction: false,
        };
        self.bitmap(bits, symbol, request, limits, work)
    }
    pub(crate) fn region(
        &mut self,
        bits: &mut Bits<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement<'_>,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let Placement {
            size,
            instances,
            config,
            identifiers,
        } = placement;
        work.operations(instances, limits)?;
        let overhead = self.bytes();
        if overhead > limits.decoded_bytes {
            return Err(Error::limit("JBIG2 Huffman text contexts exceed budget"));
        }
        let mut local = *limits;
        local.decoded_bytes -= overhead;
        let mut bitmap = Bitmap::new(size.0, size.1, config.default_black, &local)?;
        local.decoded_bytes = local.decoded_bytes.saturating_sub(bitmap.data.len());
        // Model::decode accounts for its own contexts.
        local.decoded_bytes += self.model.as_ref().map_or(0, Model::context_bytes);
        let mut strip = -self.number(2, bits)? * config.strips;
        let mut first = 0i64;
        let mut count = 0;
        while count < instances {
            strip = crate::jbig2::text::add(strip, self.number(2, bits)? * config.strips)?;
            first = crate::jbig2::text::add(first, self.number(0, bits)?)?;
            let mut s = first;
            loop {
                let t = crate::jbig2::text::add(
                    strip,
                    i64::from(
                        bits.read(
                            u8::try_from(config.strips.trailing_zeros())
                                .map_err(|_| Error::syntax("invalid JBIG2 strip size"))?,
                        )?,
                    ),
                )?;
                let id = match identifiers {
                    Identifiers::Table(table) => usize::try_from(table.required(bits)?)
                        .map_err(|_| Error::syntax("negative JBIG2 symbol identifier"))?,
                    Identifiers::Fixed(n) => bits.read(n)? as usize,
                };
                let symbol = symbols
                    .get(id)
                    .ok_or_else(|| Error::syntax("JBIG2 Huffman symbol identifier out of range"))?;
                let refined = self.refined(bits, symbol, &local, work)?;
                let glyph = refined.as_ref().unwrap_or(symbol);
                work.pixels(u64::from(glyph.width) * u64::from(glyph.height), limits)?;
                let (x, y) = config.origin(s, t, glyph)?;
                bitmap.combine_signed(glyph, x, y, config.operator);
                s = crate::jbig2::text::add(s, config.advance(glyph))?;
                count += 1;
                let Some(delta) = self.tables[1].decode(bits)? else {
                    break;
                };
                if count >= instances {
                    return Err(Error::syntax("excess JBIG2 Huffman symbol instances"));
                }
                s = crate::jbig2::text::add(s, delta + config.offset)?;
            }
        }
        Ok(bitmap)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Identifiers<'a> {
    Table(&'a Table),
    Fixed(u8),
}
pub(crate) struct Placement<'a> {
    pub(crate) size: (u32, u32),
    pub(crate) instances: usize,
    pub(crate) config: Config,
    pub(crate) identifiers: Identifiers<'a>,
}

fn dimension(base: u32, delta: i64) -> Result<u32> {
    u32::try_from(i64::from(base) + delta)
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::syntax("invalid JBIG2 refined symbol dimension"))
}

/// Run-coded identifier lengths, 7.4.3.1.7 and Table 29.
fn identifiers(
    bits: &mut Bits<'_>,
    count: usize,
    limits: &Limits,
    work: &mut Work,
) -> Result<Table> {
    if count == 0 || count > limits.value_nodes || count.saturating_mul(64) > limits.decoded_bytes {
        return Err(Error::limit(
            "JBIG2 Huffman identifier table exceeds budget",
        ));
    }
    let mut runs = Vec::with_capacity(35);
    for n in 0..35 {
        runs.push((bits.small(4)?, 0, n, 0));
    }
    let runs = Table::new(&runs)?;
    let mut lengths = Vec::with_capacity(count);
    while lengths.len() < count {
        work.operations(1, limits)?;
        let code = runs.required(bits)?;
        let (length, repeat) = match code {
            0..=31 => (
                u8::try_from(code).map_err(|_| Error::syntax("invalid JBIG2 code length"))?,
                1usize,
            ),
            32 => (
                *lengths.last().ok_or_else(|| {
                    Error::syntax("JBIG2 Huffman length repeat lacks predecessor")
                })?,
                bits.read(2)? as usize + 3,
            ),
            33 => (0, bits.read(3)? as usize + 3),
            34 => (0, bits.read(7)? as usize + 11),
            _ => return Err(Error::syntax("invalid JBIG2 Huffman length run")),
        };
        if repeat > count - lengths.len() {
            return Err(Error::syntax("JBIG2 Huffman length run exceeds symbols"));
        }
        lengths.resize(lengths.len() + repeat, length);
    }
    bits.align();
    let parameters: Vec<_> = lengths
        .iter()
        .enumerate()
        .map(|(n, &length)| {
            Ok((
                length,
                0,
                i64::try_from(n).map_err(|_| Error::limit("JBIG2 symbol identifier overflow"))?,
                0,
            ))
        })
        .collect::<Result<_>>()?;
    Table::new(&parameters)
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    size: (u32, u32),
    symbols: &[Rc<Bitmap>],
    custom: &[Rc<Table>],
    limits: &Limits,
    work: &mut Work,
) -> Result<Bitmap> {
    work.operations(crate::jbig2::huffman::STANDARD_ROWS, limits)?;
    let flags = reader.u16()?;
    let mut procedure = Procedure::read(reader, flags, custom, limits)?;
    let instances = reader.u32()? as usize;
    let base = match reader.position() {
        Position::Buffer(n) => n,
        _ => 0,
    };
    let mut bits = Bits::new(reader.rest(), base);
    let identifiers = if symbols.is_empty() {
        if instances != 0 {
            return Err(Error::syntax("JBIG2 Huffman text region has no symbols"));
        }
        for _ in 0..35 {
            bits.read(4)?;
        }
        bits.align();
        None
    } else {
        Some(identifiers(&mut bits, symbols.len(), limits, work)?)
    };
    let mut local = *limits;
    local.decoded_bytes = local
        .decoded_bytes
        .checked_sub(identifiers.as_ref().map_or(0, Table::bytes))
        .ok_or_else(|| Error::limit("JBIG2 Huffman identifier table exceeds byte budget"))?;
    let bitmap = procedure.region(
        &mut bits,
        symbols,
        Placement {
            size,
            instances,
            config: Config::from_flags(flags),
            identifiers: identifiers
                .as_ref()
                .map_or(Identifiers::Fixed(0), Identifiers::Table),
        },
        &local,
        work,
    )?;
    bits.finish()?;
    Ok(bitmap)
}
