//! Arithmetic direct-coded symbol dictionaries (T.88 clauses 6.5 and 7.4.2).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    generic::{self, Template},
    integer::Integer,
    reader::Reader,
    tags,
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

pub(super) struct Dictionary {
    pub(super) symbols: Vec<Rc<Bitmap>>,
    pub(super) bytes: usize,
}

pub(super) fn decode(
    reader: &mut Reader<'_>,
    imported: &[Rc<Bitmap>],
    limits: &Limits,
    work: &mut Work,
) -> Result<Dictionary> {
    let template = read_template(reader)?;
    let exported = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 exported symbol count overflow"))?;
    let added = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 new symbol count overflow"))?;
    let total = imported
        .len()
        .checked_add(added)
        .filter(|&n| n <= limits.value_nodes && n <= limits.objects)
        .ok_or_else(|| Error::limit("JBIG2 symbol count exceeds budget"))?;
    if exported > total {
        return Err(Error::syntax(
            "JBIG2 exported symbol count exceeds dictionary",
        ));
    }
    let overhead = total
        .checked_mul(2 * size_of::<Rc<Bitmap>>() + size_of::<Bitmap>() + 2 * size_of::<usize>())
        .and_then(|n| n.checked_add(template.context_bytes() + 3 * size_of::<Integer>()))
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or_else(|| Error::limit("JBIG2 symbol tables exceed byte budget"))?;
    let mut bitmap_limits = *limits;
    bitmap_limits.decoded_bytes -= overhead;
    let base = match reader.position() {
        Position::Buffer(p) => p,
        _ => 0,
    };
    let mut decoder = Decoder::new(reader.rest(), base)?;
    let mut heights = Integer::new();
    let mut widths = Integer::new();
    let mut contexts = vec![Context::default(); template.context_count()];
    let mut symbols = Vec::with_capacity(total);
    symbols.extend(imported.iter().cloned());
    let mut height = 0i64;
    let mut bytes = 0;
    while symbols.len() < total {
        work.operations(1, limits)?;
        height = height
            .checked_add(heights.required(&mut decoder)?)
            .filter(|&n| n > 0 && n <= i64::from(u32::MAX))
            .ok_or_else(|| Error::syntax("invalid JBIG2 symbol height"))?;
        let height =
            u32::try_from(height).map_err(|_| Error::syntax("invalid JBIG2 symbol height"))?;
        let mut width = 0i64;
        while let Some(delta) = widths.decode(&mut decoder)? {
            if symbols.len() == total {
                return Err(Error::syntax("too many JBIG2 new symbols"));
            }
            width = width
                .checked_add(delta)
                .filter(|&n| n > 0 && n <= i64::from(u32::MAX))
                .ok_or_else(|| Error::syntax("invalid JBIG2 symbol width"))?;
            let width =
                u32::try_from(width).map_err(|_| Error::syntax("invalid JBIG2 symbol width"))?;
            super::bitmap::layout(width, height, &bitmap_limits)?;
            let pixel_work = u64::from(width) * u64::from(height);
            work.pixels(pixel_work, limits)?;
            let bitmap = generic::decode_arithmetic(
                width,
                height,
                &template,
                false,
                &mut decoder,
                &mut contexts,
                &bitmap_limits,
            )?;
            bytes += bitmap.data.len();
            bitmap_limits.decoded_bytes -= bitmap.data.len();
            symbols.push(Rc::new(bitmap));
        }
    }
    let selected = select(&symbols, exported, &mut decoder, limits, work)?;
    Ok(Dictionary {
        symbols: selected,
        bytes: bytes + overhead,
    })
}

fn read_template(reader: &mut Reader<'_>) -> Result<Template> {
    let flags = reader.u16()?;
    if flags & 0xe000 != 0 {
        return Err(Error::syntax("reserved JBIG2 symbol dictionary flags"));
    }
    if flags & 1 != 0 {
        return Err(Error::unsupported(
            tags::SYMBOL_DICTIONARY,
            "JBIG2 Huffman symbol dictionaries are not implemented",
        ));
    }
    if flags & 2 != 0 {
        return Err(Error::unsupported(
            tags::SYMBOL_REFINEMENT,
            "JBIG2 refinement and aggregate symbols are not implemented",
        ));
    }
    if flags & 0x300 != 0 {
        return Err(Error::unsupported(
            tags::SYMBOL_CONTEXT,
            "JBIG2 retained symbol coding contexts are not implemented",
        ));
    }
    Template::read(
        reader,
        u8::try_from((flags >> 10) & 3)
            .map_err(|_| Error::syntax("invalid JBIG2 symbol template"))?,
    )
}

fn select(
    symbols: &[Rc<Bitmap>],
    exported: usize,
    decoder: &mut Decoder<'_>,
    limits: &Limits,
    work: &mut Work,
) -> Result<Vec<Rc<Bitmap>>> {
    let mut exports = Integer::new();
    let mut selected = Vec::with_capacity(exported);
    let mut index = 0;
    let mut export = false;
    while index < symbols.len() {
        work.operations(1, limits)?;
        let length = usize::try_from(exports.required(decoder)?)
            .ok()
            .filter(|&n| n <= symbols.len() - index)
            .ok_or_else(|| Error::syntax("invalid JBIG2 symbol export run"))?;
        if export {
            if length > exported.saturating_sub(selected.len()) {
                return Err(Error::syntax("too many JBIG2 exported symbols"));
            }
            selected.extend(symbols[index..index + length].iter().cloned());
        }
        export = !export;
        index += length;
    }
    if selected.len() != exported {
        return Err(Error::syntax("JBIG2 exported symbol count mismatch"));
    }
    Ok(selected)
}
