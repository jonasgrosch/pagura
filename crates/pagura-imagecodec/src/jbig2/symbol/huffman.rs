//! Huffman symbol dictionaries (T.88 6.5 and 7.4.2).
use crate::budget::Budget;
use crate::jbig2::{
    Bitmap,
    bitmap::{self, Fill, charge_pixels, set},
    huffman::{self, Oob, Selector, Table},
    mmr::{self, Eofb},
    refinement::{Model, Parameters},
    symbol::{Dictionary, Retained, Settings, SymbolFlags, positive, release_symbols},
    text::Config,
    text::huffman::{Aggregation, Identifiers, Placement},
};
use pagura_core::bytes::{BitCursor, Cursor};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use std::rc::Rc;

struct Header {
    settings: Settings,
    flags: SymbolFlags,
    heights: Rc<Table>,
    widths: Rc<Table>,
    sizes: Rc<Table>,
    instances: Rc<Table>,
    parameters: Option<Parameters>,
    exported: usize,
    added: usize,
}
impl Header {
    fn read(
        cursor: &mut Cursor<'_>,
        flags: SymbolFlags,
        custom: &[Rc<Table>],
        budget: &mut Budget,
    ) -> Result<Self> {
        debug_assert!(flags.huffman(), "arithmetic symbol dictionary");
        // Huffman dictionaries have no generic template, and only
        // aggregation uses instance tables, contexts and a refinement
        // template.
        let aggregation_only = SymbolFlags::INSTANCE_TABLE
            | SymbolFlags::CONTEXTS_USED
            | SymbolFlags::CONTEXTS_RETAINED
            | SymbolFlags::REFINEMENT_TEMPLATE;
        if flags.0 & (SymbolFlags::TEMPLATE | SymbolFlags::RESERVED) != 0
            || !flags.aggregate() && flags.0 & aggregation_only != 0
        {
            return Err(Error::syntax("invalid JBIG2 Huffman dictionary flags"));
        }
        let mut select = Selector::new(custom, budget);
        let heights = select.select(
            (flags.0 >> SymbolFlags::HEIGHT_TABLE_SHIFT) & 3,
            &[4, 5],
            Oob::Without,
        )?;
        let widths = select.select(
            (flags.0 >> SymbolFlags::WIDTH_TABLE_SHIFT) & 3,
            &[2, 3],
            Oob::With,
        )?;
        let sizes = select.select(
            Selector::one_bit(flags.0 & SymbolFlags::SIZE_TABLE != 0),
            &[1],
            Oob::Without,
        )?;
        let instances = select.select(
            Selector::one_bit(flags.0 & SymbolFlags::INSTANCE_TABLE != 0),
            &[1],
            Oob::Without,
        )?;
        select.finish()?;
        let parameters = flags
            .refinement()
            .map(|template| Parameters::read(cursor, template))
            .transpose()?;
        Ok(Self {
            settings: Settings::new(flags, None, parameters),
            flags,
            heights,
            widths,
            sizes,
            instances,
            parameters,
            exported: u32_to_usize(cursor.u32()?),
            added: u32_to_usize(cursor.u32()?),
        })
    }
}
struct Builder<'a> {
    header: Header,
    symbols: Vec<Rc<Bitmap>>,
    aggregation: Option<Aggregation>,
    limits: &'a Limits,
    total: usize,
    /// The width of the symbol identifiers of aggregates, at most 32.
    id_bits: u32,
}
impl<'a> Builder<'a> {
    fn new(
        header: Header,
        imported: &[Rc<Bitmap>],
        retained: Option<&Retained>,
        limits: &'a Limits,
        budget: &mut Budget,
    ) -> Result<Self> {
        let total = imported
            .len()
            .checked_add(header.added)
            .ok_or_else(|| Error::limit("JBIG2 symbol count overflow"))?;
        if header.exported > total {
            return Err(Error::syntax("JBIG2 export count exceeds symbols"));
        }
        let mut symbols = Vec::new();
        budget.reserve(&mut symbols, total)?;
        symbols.extend(imported.iter().cloned());
        let old = if header.flags.contexts_used() {
            let source = retained
                .ok_or_else(|| Error::syntax("missing retained JBIG2 refinement contexts"))?;
            header.settings.check(source)?;
            Some(
                source
                    .refinement
                    .as_deref()
                    .ok_or_else(|| Error::syntax("missing retained JBIG2 refinement contexts"))?,
            )
        } else {
            None
        };
        let aggregation = header
            .parameters
            .map(|p| {
                let model = Model::new(p, old, budget)?;
                Aggregation::new(model, budget)
            })
            .transpose()?;
        let id_bits = total.saturating_sub(1).bit_width().max(1);
        if id_bits > 32 {
            return Err(Error::limit("JBIG2 symbol identifier width overflow"));
        }
        Ok(Self {
            header,
            symbols,
            aggregation,
            limits,
            total,
            id_bits,
        })
    }
    fn class(&mut self, bits: &mut BitCursor<'_>, height: u32, budget: &mut Budget) -> Result<()> {
        let mut width = 0i64;
        let mut class = Vec::new();
        let mut collective_width = 0u32;
        loop {
            budget.work(1)?;
            let Some(delta) = self.header.widths.decode(bits)? else {
                break;
            };
            if self.symbols.len() + class.len() >= self.total {
                return Err(Error::syntax("excess JBIG2 dictionary symbols"));
            }
            width += delta;
            let size = (positive(width, "invalid JBIG2 symbol width")?, height);
            let Some(aggregation) = &mut self.aggregation else {
                collective_width = collective_width
                    .checked_add(size.0)
                    .ok_or_else(|| Error::limit("JBIG2 collective width overflow"))?;
                budget.push(&mut class, size.0)?;
                continue;
            };
            // Refinement and aggregate coding (T.88 6.5.8.2).
            let count = usize::try_from(self.header.instances.required(bits)?)
                .ok()
                .filter(|&n| n != 0)
                .ok_or_else(|| Error::syntax("invalid JBIG2 aggregation count"))?;
            let glyph = if count == 1 {
                let id = u32_to_usize(bits.bits(self.id_bits)?);
                let symbol = self
                    .symbols
                    .get(id)
                    .ok_or_else(|| Error::syntax("JBIG2 aggregation identifier out of range"))?;
                aggregation.single(bits, symbol, size, self.limits, budget)?
            } else {
                aggregation.region(
                    bits,
                    &self.symbols,
                    Placement {
                        size,
                        instances: count,
                        config: Config::aggregation(),
                        identifiers: Identifiers::Fixed(self.id_bits),
                    },
                    self.limits,
                    budget,
                )?
            };
            self.symbols.push(bitmap::share(glyph, budget)?);
        }
        if self.aggregation.is_none() {
            if class.is_empty() {
                return Err(Error::syntax("empty JBIG2 symbol height class"));
            }
            let collective = collective(
                bits,
                (collective_width, height),
                &self.header.sizes,
                self.limits,
                budget,
            )?;
            let mut x_offset = 0;
            for &width in &class {
                let glyph = extract(&collective, x_offset, width, self.limits, budget)?;
                self.symbols.push(bitmap::share(glyph, budget)?);
                x_offset += width;
            }
            collective.release(budget);
        }
        budget.release::<u32>(class.capacity());
        Ok(())
    }
    fn finish(self, bits: &mut BitCursor<'_>, budget: &mut Budget) -> Result<Dictionary> {
        let output = exports(bits, &self.symbols, self.header.exported, budget)?;
        huffman::finish(bits)?;
        release_symbols(self.symbols, budget);
        let Header {
            settings,
            flags,
            heights,
            widths,
            sizes,
            instances,
            ..
        } = self.header;
        for table in [heights, widths, sizes, instances] {
            huffman::unshare(table, budget);
        }
        let contexts = if flags.contexts_retained() {
            Some(Retained {
                settings,
                generic: Vec::new(),
                refinement: self
                    .aggregation
                    .map(|aggregation| aggregation.into_refinement_contexts(budget)),
            })
        } else {
            if let Some(aggregation) = self.aggregation {
                aggregation.release(budget);
            }
            None
        };
        Ok(Dictionary {
            symbols: output,
            contexts,
        })
    }
}

/// Decodes a Huffman symbol dictionary segment's data after its `flags`
/// (T.88 7.4.2).
pub(crate) fn decode(
    cursor: &mut Cursor<'_>,
    flags: SymbolFlags,
    imported: &[Rc<Bitmap>],
    custom: &[Rc<Table>],
    retained: Option<&Retained>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Dictionary> {
    budget.work(huffman::STANDARD_ROWS)?;
    let header = Header::read(cursor, flags, custom, budget)?;
    let mut builder = Builder::new(header, imported, retained, limits, budget)?;
    let mut bits = huffman::bits(cursor)?;
    let mut height = 0i64;
    while builder.symbols.len() < builder.total {
        budget.work(1)?;
        height += builder.header.heights.required(&mut bits)?;
        let height = positive(height, "invalid JBIG2 symbol height")?;
        builder.class(&mut bits, height, budget)?;
    }
    builder.finish(&mut bits, budget)
}
fn collective(
    bits: &mut BitCursor<'_>,
    size: (u32, u32),
    sizes: &Table,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let bytes = usize::try_from(sizes.required(bits)?)
        .map_err(|_| Error::syntax("negative JBIG2 collective data size"))?;
    let (width, height) = size;
    let (_, raw_size) = bitmap::layout(width, height, limits)?;
    // Decoding the collective bitmap and extracting its symbols.
    charge_pixels(
        budget,
        (u64::from(width) * u64::from(height)).saturating_mul(2),
    )?;
    let mut data = huffman::bytes(bits, if bytes == 0 { raw_size } else { bytes })?;
    if bytes != 0 {
        let bitmap = mmr::decode(&mut data, size, Eofb::Optional, limits, budget)?;
        if !data.at_end() {
            return Err(data.error("trailing JBIG2 MMR data"));
        }
        return Ok(bitmap);
    }
    // Uncompressed rows, whose padding bits must be zero (T.88 6.5.9).
    let mut bitmap = Bitmap::new(width, height, Fill::White, limits, budget)?;
    let raw = data.take_rest();
    let used = width % 8;
    let padding = if used == 0 { 0 } else { u8::MAX >> used };
    if raw
        .chunks(bitmap.stride)
        .any(|row| row.last().is_some_and(|last| last & padding != 0))
    {
        return Err(Error::syntax("nonzero JBIG2 raw symbol padding"));
    }
    bitmap.data.copy_from_slice(raw);
    Ok(bitmap)
}
fn exports(
    bits: &mut BitCursor<'_>,
    symbols: &[Rc<Bitmap>],
    expected: usize,
    budget: &mut Budget,
) -> Result<Vec<Rc<Bitmap>>> {
    let table = Table::standard(1, budget)?;
    let mut output = Vec::new();
    budget.reserve(&mut output, expected)?;
    let mut index = 0;
    let mut export = false;
    while index < symbols.len() {
        budget.work(1)?;
        let count = usize::try_from(table.required(bits)?)
            .map_err(|_| Error::syntax("invalid JBIG2 export run"))?;
        if count > symbols.len() - index {
            return Err(Error::syntax("JBIG2 export run exceeds symbols"));
        }
        if export && count > expected - output.len() {
            return Err(Error::syntax("too many JBIG2 exported symbols"));
        }
        if export {
            output.extend(symbols[index..index + count].iter().cloned());
        }
        index += count;
        export = !export;
    }
    if output.len() != expected {
        return Err(Error::syntax("incorrect JBIG2 exported symbol count"));
    }
    table.release(budget);
    Ok(output)
}
fn extract(
    collective: &Bitmap,
    offset: u32,
    width: u32,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let mut bitmap = Bitmap::new(width, collective.height, Fill::White, limits, budget)?;
    for y in 0..i64::from(collective.height) {
        let source = collective.row(y);
        if let Some(target) = bitmap.row_mut(y) {
            for x in 0..width {
                set(target, u32_to_usize(x), source.get(i64::from(offset + x)));
            }
        }
    }
    Ok(bitmap)
}
