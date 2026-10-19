//! Huffman text regions and aggregation (T.88 6.4 and 7.4.3).
use crate::budget::Budget;
use crate::jbig2::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::charge_pixels,
    generic::Prediction,
    huffman::{self, Line, Oob, Selector, Table, line, small},
    refinement::{Model, Parameters, Request},
    text::{Config, TextFlags, add, dimension},
};
use pagura_core::bytes::{BitCursor, Cursor};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use std::rc::Rc;

/// The tables of Huffman text or of aggregate coding (T.88 7.4.3.1.6).
struct Tables {
    /// SBHUFFFS: the first S coordinate of a strip.
    fs: Rc<Table>,
    /// SBHUFFDS: the S difference to the next instance, or out-of-band.
    ds: Rc<Table>,
    /// SBHUFFDT: the T difference to the next strip.
    dt: Rc<Table>,
    /// SBHUFFRDW and SBHUFFRDH: the refinement width and height difference.
    rdw: Rc<Table>,
    rdh: Rc<Table>,
    /// SBHUFFRDX and SBHUFFRDY: the refinement offsets.
    rdx: Rc<Table>,
    rdy: Rc<Table>,
    /// SBHUFFRSIZE: the size of the refinement data.
    rsize: Rc<Table>,
}

/// The table selections of a Huffman text region (T.88 7.4.3.1.2).
impl Tables {
    /// Drops the tables, releasing those built for this segment.
    fn release(self, budget: &mut Budget) {
        let Self {
            fs,
            ds,
            dt,
            rdw,
            rdh,
            rdx,
            rdy,
            rsize,
        } = self;
        for table in [fs, ds, dt, rdw, rdh, rdx, rdy, rsize] {
            huffman::unshare(table, budget);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selections(u16);

impl Selections {
    /// Two-bit fields: SBHUFFFS, SBHUFFDS, SBHUFFDT, SBHUFFRDW,
    /// SBHUFFRDH, SBHUFFRDX and SBHUFFRDY.
    const FS_SHIFT: u32 = 0;
    const DS_SHIFT: u32 = 2;
    const DT_SHIFT: u32 = 4;
    const RDW_SHIFT: u32 = 6;
    const RDH_SHIFT: u32 = 8;
    const RDX_SHIFT: u32 = 10;
    const RDY_SHIFT: u32 = 12;
    /// Bit 14, SBHUFFRSIZE: a user table for refinement data sizes.
    const RSIZE: u16 = 0x4000;
    /// Bits 6–14: the refinement tables, which unrefined text leaves zero.
    const REFINEMENT: u16 = 0x7fc0;
    /// Bit 15: reserved.
    const RESERVED: u16 = 0x8000;

    const fn field(self, shift: u32) -> u16 {
        (self.0 >> shift) & 3
    }
}

/// Huffman decoding of symbol instances with their tables.
struct Procedure {
    tables: Tables,
}

/// Refinement and aggregate coding of a Huffman symbol dictionary (T.88
/// 6.5.8.2), which always refines.
pub(crate) struct Aggregation {
    procedure: Procedure,
    model: Model,
}

impl Aggregation {
    /// Aggregate coding with its fixed tables (T.88 6.5.8.2.2).
    pub(crate) fn new(model: Model, budget: &mut Budget) -> Result<Self> {
        let mut table = |number| Table::standard(number, budget).map(Rc::new);
        let tables = Tables {
            fs: table(6)?,
            ds: table(8)?,
            dt: table(11)?,
            rdw: table(15)?,
            rdh: table(15)?,
            rdx: table(15)?,
            rdy: table(15)?,
            rsize: table(1)?,
        };
        Ok(Self {
            procedure: Procedure { tables },
            model,
        })
    }

    /// The refinement contexts, for a later dictionary to reuse; the
    /// tables are released.
    pub(crate) fn into_refinement_contexts(self, budget: &mut Budget) -> Vec<Context> {
        self.procedure.tables.release(budget);
        self.model.contexts
    }

    /// Drops the tables and the model, releasing their memory.
    pub(crate) fn release(self, budget: &mut Budget) {
        self.procedure.tables.release(budget);
        self.model.release(budget);
    }

    /// Decodes a refinement of one `symbol` (T.88 6.5.8.2.2).
    pub(crate) fn single(
        &mut self,
        bits: &mut BitCursor<'_>,
        symbol: &Bitmap,
        size: (u32, u32),
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let tables = &self.procedure.tables;
        let request = Request {
            width: size.0,
            height: size.1,
            dx: tables.rdx.required(bits)?,
            dy: tables.rdy.required(bits)?,
            prediction: Prediction::Off,
        };
        self.procedure
            .bitmap(bits, symbol, request, &mut self.model, limits, budget)
    }

    /// Decodes an aggregate of several symbol instances.
    pub(crate) fn region(
        &mut self,
        bits: &mut BitCursor<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement<'_>,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        self.procedure.region(
            bits,
            symbols,
            placement,
            Some(&mut self.model),
            limits,
            budget,
        )
    }
}

impl Procedure {
    fn read(
        cursor: &mut Cursor<'_>,
        flags: TextFlags,
        custom: &[Rc<Table>],
        budget: &mut Budget,
    ) -> Result<Self> {
        let selections = Selections(cursor.u16()?);
        if selections.0 & Selections::RESERVED != 0
            || flags.refinement().is_none() && selections.0 & Selections::REFINEMENT != 0
        {
            return Err(Error::syntax("invalid JBIG2 text Huffman flags"));
        }
        let mut select = Selector::new(custom, budget);
        let refinement = [14, 15];
        let tables = Tables {
            fs: select.select(
                selections.field(Selections::FS_SHIFT),
                &[6, 7],
                Oob::Without,
            )?,
            ds: select.select(
                selections.field(Selections::DS_SHIFT),
                &[8, 9, 10],
                Oob::With,
            )?,
            dt: select.select(
                selections.field(Selections::DT_SHIFT),
                &[11, 12, 13],
                Oob::Without,
            )?,
            rdw: select.select(
                selections.field(Selections::RDW_SHIFT),
                &refinement,
                Oob::Without,
            )?,
            rdh: select.select(
                selections.field(Selections::RDH_SHIFT),
                &refinement,
                Oob::Without,
            )?,
            rdx: select.select(
                selections.field(Selections::RDX_SHIFT),
                &refinement,
                Oob::Without,
            )?,
            rdy: select.select(
                selections.field(Selections::RDY_SHIFT),
                &refinement,
                Oob::Without,
            )?,
            rsize: select.select(
                Selector::one_bit(selections.0 & Selections::RSIZE != 0),
                &[1],
                Oob::Without,
            )?,
        };
        select.finish()?;
        Ok(Self { tables })
    }
    /// The refinement of an instance of `symbol` if `model` refines and the
    /// instance's refinement bit asks for it (T.88 6.4.11).
    fn refined(
        &self,
        bits: &mut BitCursor<'_>,
        symbol: &Bitmap,
        model: Option<&mut Model>,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Option<Bitmap>> {
        let Some(model) = model else {
            return Ok(None);
        };
        if !bits.bit()? {
            return Ok(None);
        }
        let dw = self.tables.rdw.required(bits)?;
        let dh = self.tables.rdh.required(bits)?;
        let request = Request {
            width: dimension(symbol.width, dw)?,
            height: dimension(symbol.height, dh)?,
            dx: (dw >> 1) + self.tables.rdx.required(bits)?,
            dy: (dh >> 1) + self.tables.rdy.required(bits)?,
            prediction: Prediction::Off,
        };
        self.bitmap(bits, symbol, request, model, limits, budget)
            .map(Some)
    }

    /// Decodes the arithmetic-coded refinement that follows its size.
    fn bitmap(
        &self,
        bits: &mut BitCursor<'_>,
        symbol: &Bitmap,
        request: Request,
        model: &mut Model,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let count = usize::try_from(self.tables.rsize.required(bits)?)
            .map_err(|_| Error::syntax("negative JBIG2 refinement data size"))?;
        let mut data = huffman::bytes(bits, count)?;
        model.decode(
            &mut Decoder::new(&mut data)?,
            symbol,
            request,
            limits,
            budget,
        )
    }

    /// Decodes the instances of a text region or aggregate (T.88 6.4.5),
    /// refining them with `model` if given.
    fn region(
        &self,
        bits: &mut BitCursor<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement<'_>,
        mut model: Option<&mut Model>,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let Placement {
            size,
            instances,
            config,
            identifiers,
        } = placement;
        budget.work(instances)?;
        let mut bitmap = Bitmap::new(size.0, size.1, config.fill, limits, budget)?;
        let mut strip = -self.tables.dt.required(bits)? * config.strips;
        let mut first = 0i64;
        let mut count = 0;
        while count < instances {
            strip = add(strip, self.tables.dt.required(bits)? * config.strips)?;
            first = add(first, self.tables.fs.required(bits)?)?;
            let mut s = first;
            loop {
                let t = add(strip, i64::from(bits.bits(config.strips.trailing_zeros())?))?;
                let id = match identifiers {
                    Identifiers::Table(table) => usize::try_from(table.required(bits)?)
                        .map_err(|_| Error::syntax("negative JBIG2 symbol identifier"))?,
                    Identifiers::Fixed(n) => u32_to_usize(bits.bits(n)?),
                };
                let symbol = symbols
                    .get(id)
                    .ok_or_else(|| Error::syntax("JBIG2 Huffman symbol identifier out of range"))?;
                let refined = self.refined(bits, symbol, model.as_deref_mut(), limits, budget)?;
                let glyph = refined.as_ref().unwrap_or(symbol);
                charge_pixels(budget, u64::from(glyph.width) * u64::from(glyph.height))?;
                let (x, y) = config.origin(s, t, glyph)?;
                bitmap.combine_signed(glyph, x, y, config.operator);
                s = add(s, config.advance(glyph))?;
                if let Some(refined) = refined {
                    refined.release(budget);
                }
                count += 1;
                let Some(delta) = self.tables.ds.decode(bits)? else {
                    break;
                };
                if count >= instances {
                    return Err(Error::syntax("excess JBIG2 Huffman symbol instances"));
                }
                s = add(s, delta + config.offset)?;
            }
        }
        Ok(bitmap)
    }
}

/// How symbol identifiers are coded: with the table of a text region
/// (T.88 7.4.3.1.7), or in a fixed number of bits in an aggregate.
#[derive(Clone, Copy)]
pub(crate) enum Identifiers<'a> {
    Table(&'a Table),
    Fixed(u32),
}
pub(crate) struct Placement<'a> {
    pub(crate) size: (u32, u32),
    pub(crate) instances: usize,
    pub(crate) config: Config,
    pub(crate) identifiers: Identifiers<'a>,
}

/// Run-coded identifier lengths (T.88 7.4.3.1.7 and Table 29).
fn identifiers(bits: &mut BitCursor<'_>, count: usize, budget: &mut Budget) -> Result<Table> {
    let mut lengths = Vec::new();
    budget.reserve(&mut lengths, count)?;
    // The prefix lengths of the 35 run codes (T.88 7.4.3.1.7).
    let mut runs = [line(0, 0, 0); 35];
    for (n, run) in (0..).zip(&mut runs) {
        *run = line(small(bits, 4)?, 0, n);
    }
    let runs = Table::new(&runs, budget)?;
    while lengths.len() < count {
        budget.work(1)?;
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
                u32_to_usize(bits.bits(2)?) + 3,
            ),
            33 => (0, u32_to_usize(bits.bits(3)?) + 3),
            34 => (0, u32_to_usize(bits.bits(7)?) + 11),
            _ => return Err(Error::syntax("invalid JBIG2 Huffman length run")),
        };
        if repeat > count - lengths.len() {
            return Err(Error::syntax("JBIG2 Huffman length run exceeds symbols"));
        }
        lengths.resize(lengths.len() + repeat, length);
    }
    bits.align()?;
    runs.release(budget);
    let mut parameters = Vec::new();
    budget.reserve(&mut parameters, count)?;
    for (n, &length) in (0..).zip(&lengths) {
        parameters.push(line(length, 0, n));
    }
    budget.release::<u8>(count);
    let table = Table::new(&parameters, budget)?;
    budget.release::<Line>(count);
    Ok(table)
}

/// Decodes a Huffman text region segment's data after its `flags` (T.88
/// 7.4.3).
pub(crate) fn decode(
    cursor: &mut Cursor<'_>,
    flags: TextFlags,
    size: (u32, u32),
    symbols: &[Rc<Bitmap>],
    custom: &[Rc<Table>],
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    budget.work(huffman::STANDARD_ROWS)?;
    let procedure = Procedure::read(cursor, flags, custom, budget)?;
    let mut model = flags
        .refinement()
        .map(|template| Model::new(Parameters::read(cursor, template)?, None, budget))
        .transpose()?;
    let instances = u32_to_usize(cursor.u32()?);
    let mut bits = huffman::bits(cursor)?;
    let identifiers = if symbols.is_empty() {
        if instances != 0 {
            return Err(Error::syntax("JBIG2 Huffman text region has no symbols"));
        }
        for _ in 0..35 {
            bits.bits(4)?;
        }
        bits.align()?;
        None
    } else {
        Some(identifiers(&mut bits, symbols.len(), budget)?)
    };
    let bitmap = procedure.region(
        &mut bits,
        symbols,
        Placement {
            size,
            instances,
            config: flags.config(),
            identifiers: identifiers
                .as_ref()
                .map_or(Identifiers::Fixed(0), Identifiers::Table),
        },
        model.as_mut(),
        limits,
        budget,
    )?;
    huffman::finish(&mut bits)?;
    procedure.tables.release(budget);
    if let Some(identifiers) = identifiers {
        identifiers.release(budget);
    }
    if let Some(model) = model {
        model.release(budget);
    }
    Ok(bitmap)
}
