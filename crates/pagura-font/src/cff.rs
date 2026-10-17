//! CFF containers, DICTs and INDEXes (Adobe Technical Note 5176).
use crate::GlyphId;
use crate::read::{Read, bad, cap};
use pagura_core::{Diagnostics, Limits, Name, Recovery, Result};
use pagura_raster::{Path, Transform};
use std::collections::BTreeMap;
use std::ops::Range;

mod dict;
mod predefined;
mod select;
pub(crate) mod strings;
mod type2;
mod variation;

#[derive(Debug)]
struct Private {
    subrs: Vec<Range<usize>>,
    default_width: f64,
    nominal_width: f64,
    matrix: Transform,
    parse_work: usize,
    vsindex: usize,
    random_seed: u32,
}

#[derive(Debug)]
pub(crate) struct Cff {
    glyphs: Vec<Range<usize>>,
    global: Vec<Range<usize>>,
    private: Vec<Private>,
    select: Vec<usize>,
    names: BTreeMap<Name, GlyphId>,
    cids: Option<BTreeMap<u32, GlyphId>>,
    encoding: Option<Box<[Option<Name>; 256]>>,
    matrix: Transform,
    cff2: bool,
    variations: Vec<Vec<f64>>,
}

pub(super) fn index(r: &mut Read<'_>, limits: &Limits, cff2: bool) -> Result<Vec<Range<usize>>> {
    let count = if cff2 {
        r.offset()?
    } else {
        usize::from(r.u16()?)
    };
    cap(
        count,
        limits
            .operations
            .min(limits.objects)
            .min(limits.value_nodes)
            .min(limits.decoded_bytes / size_of::<Range<usize>>()),
        r.at(),
    )?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let size = r.u8()?;
    if !(1..=4).contains(&size) {
        return Err(bad(r.at(), "invalid CFF offset size"));
    }
    let mut offsets = Vec::with_capacity(count + 1);
    let mut previous = 1;
    for i in 0..=count {
        let mut n = 0_usize;
        for _ in 0..size {
            n = n
                .checked_mul(256)
                .and_then(|n| n.checked_add(usize::from(r.u8().ok()?)))
                .ok_or_else(|| bad(r.at(), "invalid CFF offset"))?;
        }
        if n < previous || (i == 0 && n != 1) {
            return Err(bad(r.at(), "invalid CFF INDEX offsets"));
        }
        offsets.push(n - 1);
        previous = n;
    }
    let start = r.at();
    let len = *offsets
        .last()
        .ok_or_else(|| bad(r.at(), "missing CFF offset"))?;
    cap(len, limits.decoded_bytes, start)?;
    r.skip(len)?;
    Ok(offsets
        .windows(2)
        .map(|w| start + w[0]..start + w[1])
        .collect())
}

impl Cff {
    pub fn parse(data: &[u8], range: Range<usize>, limits: &Limits) -> Result<Self> {
        let Header {
            top,
            strings: string_ranges,
            global,
            cff2,
        } = header(data, range.clone(), limits)?;
        let table = Read::new(&data[range.clone()], range.start);
        if !cff2 {
            top.check(false, string_ranges.len())?;
        }
        if top.has(0x0c14) {
            return Err(pagura_core::Error::unsupported(
                crate::features::FORMAT,
                "synthetic CFF font is not supported",
            )
            .at(pagura_core::Position::Buffer(range.start)));
        }
        let mut charstrings = table.tail(top.offset(17, None)?)?;
        let glyphs = index(&mut charstrings, limits, cff2)?;
        if glyphs.is_empty() {
            return Err(bad(charstrings.at(), "CFF has no glyphs"));
        }
        cap(glyphs.len(), limits.objects, charstrings.at())?;
        if !cff2 && top.number(0x0c06, 2.0)? != 2.0 {
            return Err(pagura_core::Error::unsupported(
                crate::features::FORMAT,
                "CFF CharstringType is not Type 2",
            )
            .at(pagura_core::Position::Buffer(range.start)));
        }
        let variations = if cff2 && top.has(24) {
            variation::parse(table.tail(top.offset(24, None)?)?, limits)?
        } else {
            Vec::new()
        };
        let matrix = top.matrix(Transform::scale(0.001, 0.001))?;
        let cid = top.has(0x0c1e);
        let charset = if cff2 {
            Vec::new()
        } else {
            charset(table, top.offset(15, Some(0))?, glyphs.len(), limits)?
        };
        let mut names = BTreeMap::new();
        let mut cids = cid.then(BTreeMap::new);
        for (i, sid) in charset.iter().copied().enumerate() {
            let glyph = GlyphId(
                u32::try_from(i)
                    .map_err(|_| crate::read::limit(range.start, "glyph count overflow"))?,
            );
            if let Some(cids) = &mut cids {
                if f64::from(sid) >= top.number(0x0c22, 8720.0)? {
                    return Err(bad(range.start, "CID outside CFF CIDCount"));
                }
                if cids.insert(u32::from(sid), glyph).is_some() {
                    return Err(bad(range.start, "duplicate CFF CID"));
                }
            } else {
                let name = strings::sid(sid, &string_ranges, data, limits)?;
                if names.insert(name, glyph).is_some() {
                    return Err(bad(range.start, "duplicate CFF glyph name"));
                }
            }
        }
        let encoding = if cid || cff2 {
            None
        } else {
            Some(strings::encoding(
                table,
                top.offset(16, Some(0))?,
                &charset,
                &string_ranges,
                data,
                limits,
            )?)
        };
        let context = Context {
            cff2,
            variations: &variations,
            strings: string_ranges.len(),
        };
        let (private, select) =
            dictionaries(table, &top, data, glyphs.len(), cid, context, limits)?;
        Ok(Self {
            glyphs,
            global,
            private,
            select,
            names,
            cids,
            encoding,
            matrix,
            cff2,
            variations,
        })
    }
    pub fn glyph_count(&self) -> u32 {
        u32::try_from(self.glyphs.len()).unwrap_or(0)
    }
    pub fn matrix(&self) -> Transform {
        self.matrix
    }
    pub fn is_cid(&self) -> bool {
        self.cids.is_some()
    }
    pub fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cids.as_ref()?.get(&cid).copied()
    }
    pub fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.names.get(name).copied()
    }
    pub fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.encoding.as_deref()
    }
    pub fn outline(
        &self,
        data: &[u8],
        glyph: GlyphId,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Path> {
        Ok(type2::glyph(self, data, glyph, limits, diagnostics)?.0)
    }
    pub fn advance(&self, data: &[u8], glyph: GlyphId, limits: &Limits) -> Option<f64> {
        if self.cff2 {
            None
        } else {
            self.glyph(data, glyph, limits).ok().map(|g| g.1)
        }
    }
    fn glyph(&self, data: &[u8], glyph: GlyphId, limits: &Limits) -> Result<(Path, f64)> {
        type2::glyph(
            self,
            data,
            glyph,
            limits,
            &mut Diagnostics::new(Recovery::Strict),
        )
    }
}

fn private_dict(
    table: Read<'_>,
    top: &dict::Dict,
    limits: &Limits,
    context: Context<'_>,
    matrix: Transform,
) -> Result<Private> {
    let pair = match top.values(18) {
        Some(pair) => pair,
        None if context.cff2 => &[0.0, 0.0],
        None => return Err(bad(table.base, "missing CFF Private DICT")),
    };
    if pair.len() != 2 {
        return Err(bad(table.base, "invalid CFF Private operands"));
    }
    let size = dict::integer(pair[0], table.base)?;
    let offset = dict::integer(pair[1], table.base)?;
    cap(size, limits.operations, table.base)?;
    let mut r = table.tail(offset)?;
    let at = r.at();
    let bytes = Read::new(r.take(size)?, at);
    let private = if context.cff2 {
        dict::parse_private(bytes, limits, context.variations)?
    } else {
        dict::parse(bytes, limits)?
    };
    if !context.cff2 {
        private.check(true, 0)?;
    }
    let subrs = if private.has(19) {
        index(
            &mut table.tail(
                offset
                    .checked_add(private.offset(19, None)?)
                    .ok_or_else(|| crate::read::limit(at, "private offset overflow"))?,
            )?,
            limits,
            context.cff2,
        )?
    } else {
        Vec::new()
    };
    let parse_work = size.saturating_add(subrs.len());
    cap(parse_work, limits.operations, at)?;
    Ok(Private {
        subrs,
        default_width: private.number(20, 0.0)?,
        nominal_width: private.number(21, 0.0)?,
        matrix,
        parse_work,
        vsindex: private.offset(22, Some(0))?,
        random_seed: random_seed(private.number(0x0c13, 0.0)?),
    })
}

fn charset(table: Read<'_>, offset: usize, glyphs: usize, limits: &Limits) -> Result<Vec<u16>> {
    if offset == 0 {
        if glyphs > 229 {
            return Err(bad(table.base, "ISOAdobe charset glyph count exceeded"));
        }
        return (0..glyphs)
            .map(|i| {
                u16::try_from(i).map_err(|_| crate::read::limit(table.base, "charset overflow"))
            })
            .collect();
    }
    if offset <= 2 {
        let predefined = if offset == 1 {
            predefined::EXPERT
        } else {
            predefined::EXPERT_SUBSET
        };
        return predefined
            .get(..glyphs)
            .map(<[u16]>::to_vec)
            .ok_or_else(|| bad(table.base, "predefined CFF charset glyph count exceeded"));
    }
    let mut r = table.tail(offset)?;
    let format = r.u8()?;
    cap(glyphs, limits.value_nodes, r.at())?;
    let mut result = Vec::with_capacity(glyphs);
    result.push(0);
    while result.len() < glyphs {
        let first = r.u16()?;
        let left = match format {
            0 => 0,
            1 => u16::from(r.u8()?),
            2 => r.u16()?,
            _ => return Err(bad(r.at(), "invalid CFF charset format")),
        };
        if usize::from(left) + 1 > glyphs - result.len() || first.checked_add(left).is_none() {
            return Err(bad(r.at(), "CFF charset range exceeds glyph count"));
        }
        result.extend(first..=first + left);
    }
    Ok(result)
}

struct Header {
    top: dict::Dict,
    strings: Vec<Range<usize>>,
    global: Vec<Range<usize>>,
    cff2: bool,
}
fn header(data: &[u8], range: Range<usize>, limits: &Limits) -> Result<Header> {
    let mut r = Read::new(
        data.get(range.clone())
            .ok_or_else(|| bad(range.start, "invalid CFF table"))?,
        range.start,
    );
    let major = r.u8()?;
    if !matches!(major, 1 | 2) {
        return Err(bad(r.at(), "invalid CFF version"));
    }
    let cff2 = major == 2;
    r.u8()?;
    let header = usize::from(r.u8()?);
    let (top, strings, global) = if cff2 {
        let size = usize::from(r.u16()?);
        if header < 5 {
            return Err(bad(r.at(), "invalid CFF2 header"));
        }
        r = r.tail(header)?;
        let at = r.at();
        let top = dict::parse_cff2(Read::new(r.take(size)?, at), limits)?;
        (top, Vec::new(), index(&mut r, limits, true)?)
    } else {
        let offset_size = r.u8()?;
        if header < 4 || !(1..=4).contains(&offset_size) {
            return Err(bad(r.at(), "invalid CFF header"));
        }
        r = r.tail(header)?;
        if index(&mut r, limits, false)?.len() != 1 {
            return Err(bad(r.at(), "embedded CFF must contain one font"));
        }
        let tops = index(&mut r, limits, false)?;
        let top = tops
            .first()
            .filter(|_| tops.len() == 1)
            .ok_or_else(|| bad(r.at(), "CFF Top DICT count mismatch"))?;
        let top = dict::parse(Read::new(&data[top.clone()], top.start), limits)?;
        let strings = index(&mut r, limits, false)?;
        let global = index(&mut r, limits, false)?;
        (top, strings, global)
    };
    Ok(Header {
        top,
        strings,
        global,
        cff2,
    })
}

fn dictionaries(
    table: Read<'_>,
    top: &dict::Dict,
    data: &[u8],
    glyphs: usize,
    cid: bool,
    context: Context<'_>,
    limits: &Limits,
) -> Result<(Vec<Private>, Vec<usize>)> {
    let cff2 = context.cff2;
    let (private, select) = if cid || cff2 && top.has(0x0c24) {
        let mut array = table.tail(top.offset(0x0c24, None)?)?;
        let fonts = index(&mut array, limits, cff2)?;
        if fonts.is_empty() || fonts.len() > if cff2 { 65536 } else { 256 } {
            return Err(bad(array.at(), "invalid CFF FDArray count"));
        }
        let mut private = Vec::with_capacity(fonts.len());
        let mut remaining = *limits;
        for font in fonts {
            cap(font.len(), remaining.operations, font.start)?;
            remaining.operations = remaining.operations.saturating_sub(font.len());
            let dict = if cff2 { dict::parse_cff2 } else { dict::parse }(
                Read::new(&data[font.clone()], font.start),
                limits,
            )?;
            if !cff2 {
                dict.check(false, context.strings)?;
            }
            if cff2 && dict.has(0x0c07) {
                // Secondary outline readers disagree for this unreviewed
                // CFF2 combination. Keep it explicit until primary review.
                return Err(pagura_core::Error::unsupported(
                    crate::features::FORMAT,
                    "CFF2 Font DICT matrices are not supported",
                )
                .at(pagura_core::Position::Buffer(font.start)));
            }
            let parsed = private_dict(
                table,
                &dict,
                &remaining,
                context,
                dict.matrix(Transform::IDENTITY)?,
            )?;
            remaining.value_nodes = remaining.value_nodes.saturating_sub(parsed.subrs.len() + 1);
            remaining.operations = remaining.operations.saturating_sub(parsed.parse_work + 1);
            private.push(parsed);
        }
        let select = if cff2 && !top.has(0x0c25) && private.len() == 1 {
            vec![0; glyphs]
        } else {
            select::parse(
                table.tail(top.offset(0x0c25, None)?)?,
                glyphs,
                private.len(),
                limits,
                cff2,
            )?
        };
        (private, select)
    } else {
        (
            vec![private_dict(
                table,
                top,
                limits,
                context,
                Transform::IDENTITY,
            )?],
            vec![0; glyphs],
        )
    };
    Ok((private, select))
}

#[derive(Clone, Copy)]
struct Context<'a> {
    cff2: bool,
    variations: &'a [Vec<f64>],
    strings: usize,
}

// TN 5176 defines an initial seed, not a generator algorithm. Fold the numeric
// representation so arbitrary finite DICT numbers have reproducible seeds.
fn random_seed(value: f64) -> u32 {
    let bits = value.to_bits().to_be_bytes();
    u32::from_be_bytes([
        bits[0] ^ bits[4],
        bits[1] ^ bits[5],
        bits[2] ^ bits[6],
        bits[3] ^ bits[7],
    ])
}
