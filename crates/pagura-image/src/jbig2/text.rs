//! Arithmetic text regions without refinement (T.88 clauses 6.4 and 7.4.3).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    integer::{Integer, symbol_id},
    reader::Reader,
    tags,
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

pub(super) fn decode(
    reader: &mut Reader<'_>,
    width: u32,
    height: u32,
    symbols: &[Rc<Bitmap>],
    limits: &Limits,
    work: &mut Work,
) -> Result<Bitmap> {
    let Config {
        strips,
        operator,
        offset,
        default_black,
    } = Config::read(reader)?;
    let instances = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 text instance count overflow"))?;
    work.operations(instances, limits)?;
    if instances != 0 && symbols.is_empty() {
        return Err(Error::syntax("JBIG2 text region has no symbols"));
    }
    let context_count = symbols
        .len()
        .max(1)
        .checked_next_power_of_two()
        .ok_or_else(|| Error::limit("JBIG2 symbol identifier size overflow"))?;
    let overhead = context_count
        .checked_mul(size_of::<Context>())
        .and_then(|n| n.checked_add(4 * size_of::<Integer>()))
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or_else(|| Error::limit("JBIG2 text contexts exceed byte budget"))?;
    let mut bitmap_limits = *limits;
    bitmap_limits.decoded_bytes -= overhead;
    let mut bitmap = Bitmap::new(width, height, default_black, &bitmap_limits)?;
    let mut identifiers = vec![Context::default(); context_count];
    let mut delta_t = Integer::new();
    let mut first_s = Integer::new();
    let mut delta_s = Integer::new();
    let mut current_t = Integer::new();
    let base = match reader.position() {
        Position::Buffer(p) => p,
        _ => 0,
    };
    let mut decoder = Decoder::new(reader.rest(), base)?;
    let mut strip_t = -delta_t.required(&mut decoder)? * strips;
    let mut first = 0;
    let mut count = 0;
    while count < instances {
        strip_t = add(strip_t, delta_t.required(&mut decoder)? * strips)?;
        first = add(first, first_s.required(&mut decoder)?)?;
        let mut s = first;
        loop {
            let t = if strips == 1 {
                0
            } else {
                current_t.required(&mut decoder)?
            };
            if t < 0 || t >= strips {
                return Err(Error::syntax("invalid JBIG2 text position within strip"));
            }
            let t = add(strip_t, t)?;
            let id = symbol_id(&mut decoder, &mut identifiers)?;
            let symbol = symbols
                .get(id)
                .ok_or_else(|| Error::syntax("JBIG2 symbol identifier out of range"))?;
            work.pixels(u64::from(symbol.width) * u64::from(symbol.height), limits)?;
            bitmap.combine_signed(symbol, s, t, operator);
            count += 1;
            if count == instances {
                break;
            }
            s = add(s, i64::from(symbol.width) - 1)?;
            let Some(delta) = delta_s.decode(&mut decoder)? else {
                break;
            };
            s = add(s, delta + offset)?;
        }
    }
    Ok(bitmap)
}

struct Config {
    strips: i64,
    operator: u8,
    offset: i64,
    default_black: bool,
}

impl Config {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let flags = reader.u16()?;
        if flags & 1 != 0 {
            return Err(Error::unsupported(
                tags::TEXT_REGION,
                "JBIG2 Huffman text regions are not implemented",
            ));
        }
        if flags & 2 != 0 {
            return Err(Error::unsupported(
                tags::TEXT_REFINEMENT,
                "JBIG2 text refinement is not implemented",
            ));
        }
        if flags & 0x70 != 0x10 {
            return Err(Error::unsupported(
                tags::TEXT_LAYOUT,
                "JBIG2 text reference corner or transposition is not implemented",
            ));
        }
        let strips = 1i64 << ((flags >> 2) & 3);
        let operator = u8::try_from((flags >> 7) & 3)
            .map_err(|_| Error::syntax("invalid JBIG2 text operator"))?;
        let raw_offset = i64::from((flags >> 10) & 31);
        let offset = if raw_offset >= 16 {
            raw_offset - 32
        } else {
            raw_offset
        };
        Ok(Self {
            strips,
            operator,
            offset,
            default_black: flags & 512 != 0,
        })
    }
}

fn add(left: i64, right: i64) -> Result<i64> {
    left.checked_add(right)
        .ok_or_else(|| Error::limit("JBIG2 text coordinate overflow"))
}
