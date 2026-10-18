//! Huffman dictionaries, T.88 (02/2000), 6.5 and 7.4.2.
use crate::jbig2::{
    Bitmap,
    budget::Work,
    huffman::{Bits, Table},
    reader::Reader,
    refinement::{Model, Parameters},
    symbol::{Dictionary, Retained, Settings},
    text::Config,
    text::huffman::{self as huffman_text, Identifiers, Placement, Procedure},
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

struct Header {
    settings: Settings,
    flags: u16,
    heights: Rc<Table>,
    widths: Rc<Table>,
    sizes: Rc<Table>,
    instances: Rc<Table>,
    parameters: Option<Parameters>,
    exported: usize,
    added: usize,
}
impl Header {
    fn read(reader: &mut Reader<'_>, custom: &[Rc<Table>]) -> Result<Self> {
        let flags = reader.u16()?;
        let aggregation = flags & 2 != 0;
        if flags & 0xec00 != 0 || !aggregation && flags & 0x1380 != 0 {
            return Err(Error::syntax("invalid JBIG2 Huffman dictionary flags"));
        }
        let mut custom = custom.iter();
        let heights = huffman_text::select((flags >> 2) & 3, &[4, 5], &mut custom, false)?;
        let widths = huffman_text::select((flags >> 4) & 3, &[2, 3], &mut custom, true)?;
        let sizes = huffman_text::select(
            if flags & 64 != 0 { 3 } else { 0 },
            &[1],
            &mut custom,
            false,
        )?;
        let instances = huffman_text::select(
            if flags & 128 != 0 { 3 } else { 0 },
            &[1],
            &mut custom,
            false,
        )?;
        if custom.next().is_some() {
            return Err(Error::syntax("unused JBIG2 user Huffman table"));
        }
        let adaptive_start = reader.offset;
        let parameters = if aggregation {
            Some(Parameters::read(reader, u8::from(flags & 0x1000 != 0))?)
        } else {
            None
        };
        Ok(Self {
            settings: Settings::new(flags, &reader.data[adaptive_start..reader.offset])?,
            flags,
            heights,
            widths,
            sizes,
            instances,
            parameters,
            exported: reader.u32()? as usize,
            added: reader.u32()? as usize,
        })
    }
}
struct Builder {
    header: Header,
    symbols: Vec<Rc<Bitmap>>,
    procedure: Option<Procedure>,
    limits: Limits,
    overhead: usize,
    bytes: usize,
    total: usize,
    id_bits: u8,
}
impl Builder {
    fn new(
        header: Header,
        imported: &[Rc<Bitmap>],
        retained: Option<&Retained>,
        limits: &Limits,
    ) -> Result<Self> {
        let total = imported
            .len()
            .checked_add(header.added)
            .filter(|&n| n <= limits.value_nodes && n <= limits.objects)
            .ok_or_else(|| Error::limit("JBIG2 symbol count exceeds budget"))?;
        if header.exported > total {
            return Err(Error::syntax("JBIG2 export count exceeds symbols"));
        }
        // Reference arrays, per-symbol bitmap allocations, widths, and the four code tables coexist.
        let tables_bytes = header.heights.bytes()
            + header.widths.bytes()
            + header.sizes.bytes()
            + header.instances.bytes();
        let overhead = total
            .checked_mul(
                size_of::<Rc<Bitmap>>() * 2
                    + size_of::<Bitmap>()
                    + 2 * size_of::<usize>()
                    + size_of::<u32>(),
            )
            .and_then(|n| n.checked_add(tables_bytes))
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or_else(|| Error::limit("JBIG2 Huffman symbol tables exceed byte budget"))?;
        let mut local = *limits;
        local.decoded_bytes -= overhead;
        let old = if header.flags & 256 != 0 {
            let source = retained
                .ok_or_else(|| Error::syntax("missing retained JBIG2 refinement contexts"))?;
            header.settings.check(source)?;
            Some(
                retained
                    .and_then(|r| r.refinement.as_deref())
                    .ok_or_else(|| Error::syntax("missing retained JBIG2 refinement contexts"))?,
            )
        } else {
            None
        };
        let procedure = header
            .parameters
            .map(|p| Model::new(p, old, &local).and_then(Procedure::aggregation))
            .transpose()?;
        if let Some(p) = &procedure {
            local.decoded_bytes = local
                .decoded_bytes
                .checked_sub(p.bytes())
                .ok_or_else(|| Error::limit("JBIG2 refinement contexts exceed budget"))?;
        }
        let mut symbols = Vec::with_capacity(total);
        symbols.extend(imported.iter().cloned());
        let id_bits = u8::try_from(total.saturating_sub(1).bit_width().max(1))
            .map_err(|_| Error::limit("JBIG2 symbol identifier width overflow"))?;
        Ok(Self {
            header,
            symbols,
            procedure,
            limits: local,
            overhead,
            bytes: 0,
            total,
            id_bits,
        })
    }
    fn class(&mut self, bits: &mut Bits<'_>, height: u32, work: &mut Work) -> Result<()> {
        let mut width = 0i64;
        let mut class = Vec::with_capacity(self.total - self.symbols.len());
        let mut collective_width = 0u32;
        loop {
            work.operations(1, &self.limits)?;
            let Some(delta) = self.header.widths.decode(bits)? else {
                break;
            };
            if self.symbols.len() + class.len() >= self.total {
                return Err(Error::syntax("excess JBIG2 dictionary symbols"));
            }
            width += delta;
            let width = positive(width)?;
            if self.procedure.is_some() {
                let glyph = self.aggregate(bits, (width, height), work)?;
                self.bytes = self
                    .bytes
                    .checked_add(glyph.data.len())
                    .filter(|&n| n <= self.limits.decoded_bytes)
                    .ok_or_else(|| Error::limit("JBIG2 symbols exceed byte budget"))?;
                self.symbols.push(Rc::new(glyph));
            } else {
                collective_width = collective_width
                    .checked_add(width)
                    .ok_or_else(|| Error::limit("JBIG2 collective width overflow"))?;
                class.push(width);
            }
        }
        if self.procedure.is_none() {
            if class.is_empty() {
                return Err(Error::syntax("empty JBIG2 symbol height class"));
            }
            let mut temporary = self.limits;
            temporary.decoded_bytes = temporary.decoded_bytes.saturating_sub(self.bytes);
            let collective = collective(
                bits,
                (collective_width, height),
                &self.header.sizes,
                &temporary,
                work,
            )?;
            temporary.decoded_bytes = temporary
                .decoded_bytes
                .saturating_sub(collective.data.len());
            let mut x_offset = 0;
            for width in class {
                let glyph = extract(&collective, x_offset, width, &temporary)?;
                temporary.decoded_bytes = temporary.decoded_bytes.saturating_sub(glyph.data.len());
                self.bytes += glyph.data.len();
                self.symbols.push(Rc::new(glyph));
                x_offset += width;
            }
        }
        Ok(())
    }
    fn aggregate(
        &mut self,
        bits: &mut Bits<'_>,
        size: (u32, u32),
        work: &mut Work,
    ) -> Result<Bitmap> {
        let count = usize::try_from(self.header.instances.required(bits)?)
            .ok()
            .filter(|&n| n != 0)
            .ok_or_else(|| Error::syntax("invalid JBIG2 aggregation count"))?;
        let procedure = self
            .procedure
            .as_mut()
            .ok_or_else(|| Error::syntax("missing JBIG2 aggregation procedure"))?;
        let mut local = self.limits;
        local.decoded_bytes = local.decoded_bytes.saturating_sub(self.bytes);
        if count == 1 {
            // The refinement decoder charges its own model; the other seven tables remain live.
            local.decoded_bytes += procedure.model.as_ref().map_or(0, Model::context_bytes);
            procedure.single(bits, &self.symbols, size, self.id_bits, &local, work)
        } else {
            local.decoded_bytes += procedure.bytes();
            procedure.region(
                bits,
                &self.symbols,
                Placement {
                    size,
                    instances: count,
                    config: Config::aggregation(),
                    identifiers: Identifiers::Fixed(self.id_bits),
                },
                &local,
                work,
            )
        }
    }
    fn finish(self, bits: &mut Bits<'_>, work: &mut Work) -> Result<Dictionary> {
        let output = exports(
            bits,
            &self.symbols,
            self.header.exported,
            &self.limits,
            work,
        )?;
        bits.finish()?;
        let contexts = if self.header.flags & 512 != 0 {
            Some(Retained {
                settings: self.header.settings,
                generic: Vec::new(),
                refinement: self.procedure.and_then(|p| p.model.map(|m| m.contexts)),
            })
        } else {
            None
        };
        let retained_bytes = contexts
            .as_ref()
            .and_then(|r| r.refinement.as_ref())
            .map_or(0, |c| {
                c.len() * size_of::<crate::jbig2::arithmetic::Context>()
            });
        Ok(Dictionary {
            symbols: output,
            bytes: self.overhead + self.bytes + retained_bytes,
            contexts,
        })
    }
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    imported: &[Rc<Bitmap>],
    custom: &[Rc<Table>],
    retained: Option<&Retained>,
    limits: &Limits,
    work: &mut Work,
) -> Result<Dictionary> {
    work.operations(crate::jbig2::huffman::STANDARD_ROWS, limits)?;
    let header = Header::read(reader, custom)?;
    let mut builder = Builder::new(header, imported, retained, limits)?;
    let base = match reader.position() {
        Position::Buffer(n) => n,
        _ => 0,
    };
    let mut bits = Bits::new(reader.rest(), base);
    let mut height = 0i64;
    while builder.symbols.len() < builder.total {
        work.operations(1, limits)?;
        height += builder.header.heights.required(&mut bits)?;
        builder.class(&mut bits, positive(height)?, work)?;
    }
    builder.finish(&mut bits, work)
}
fn collective(
    bits: &mut Bits<'_>,
    size: (u32, u32),
    sizes: &Table,
    limits: &Limits,
    work: &mut Work,
) -> Result<Bitmap> {
    let bytes = usize::try_from(sizes.required(bits)?)
        .map_err(|_| Error::syntax("negative JBIG2 collective data size"))?;
    let (width, height) = size;
    let (_, raw_size) = crate::jbig2::bitmap::layout(width, height, limits)?;
    work.pixels(u64::from(width) * u64::from(height) * 2, limits)?;
    bits.align();
    let position = bits.position();
    let data = bits.take(if bytes == 0 { raw_size } else { bytes })?;
    if bytes != 0 {
        return crate::jbig2::mmr::decode(
            data,
            width,
            height,
            limits,
            match position {
                Position::Buffer(n) => n,
                _ => 0,
            },
        );
    }
    let mut bitmap = Bitmap::new(width, height, false, limits)?;
    bitmap.data.copy_from_slice(data);
    let padding = (8 - width % 8) % 8;
    if padding != 0
        && bitmap
            .data
            .chunks(bitmap.stride())
            .any(|row| row[row.len() - 1] & ((1 << padding) - 1) != 0)
    {
        return Err(Error::syntax("nonzero JBIG2 raw symbol padding"));
    }
    Ok(bitmap)
}
fn exports(
    bits: &mut Bits<'_>,
    symbols: &[Rc<Bitmap>],
    expected: usize,
    limits: &Limits,
    work: &mut Work,
) -> Result<Vec<Rc<Bitmap>>> {
    let table = Table::standard(1)?;
    let mut output = Vec::with_capacity(expected);
    let mut index = 0;
    let mut export = false;
    while index < symbols.len() {
        work.operations(1, limits)?;
        let count = usize::try_from(table.required(bits)?)
            .map_err(|_| Error::syntax("invalid JBIG2 export run"))?;
        if count > symbols.len() - index {
            return Err(Error::syntax("JBIG2 export run exceeds symbols"));
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
    Ok(output)
}
fn positive(value: i64) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::syntax("invalid JBIG2 symbol dimensions"))
}
fn extract(collective: &Bitmap, offset: u32, width: u32, limits: &Limits) -> Result<Bitmap> {
    let mut bitmap = Bitmap::new(width, collective.height, false, limits)?;
    for y in 0..collective.height {
        for x in 0..width {
            bitmap.set(x, y, collective.pixel(i64::from(offset + x), i64::from(y)));
        }
    }
    Ok(bitmap)
}
