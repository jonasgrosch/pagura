//! CFF containers, DICTs and INDEXes (Adobe Technical Note 5176).
use crate::GlyphId;
use crate::budget::Work;
use crate::memory::{encoding, names, nested, tree, vector};
use crate::read::{bad, here, slice};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};
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
    glyph_names: Vec<Option<Name>>,
    cids: Option<BTreeMap<u32, GlyphId>>,
    encoding: Option<Box<[Option<Name>; 256]>>,
    matrix: Transform,
    cff2: bool,
    variations: Vec<Vec<f64>>,
}

pub(super) fn index(
    r: &mut Cursor<'_>,
    work: &mut Work<'_>,
    cff2: bool,
) -> Result<Vec<Range<usize>>> {
    let count = if cff2 {
        u32_to_usize(r.u32()?)
    } else {
        usize::from(r.u16()?)
    };
    work.entries(count, Position::Buffer(here(r)))?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let size = r.u8()?;
    if !(1..=4).contains(&size) {
        return Err(bad(here(r), "invalid CFF offset size"));
    }
    let mut offsets = Vec::with_capacity(count + 1);
    let mut previous = 1;
    for i in 0..=count {
        let mut n = 0_usize;
        for _ in 0..size {
            n = n
                .checked_mul(256)
                .and_then(|n| n.checked_add(usize::from(r.u8().ok()?)))
                .ok_or_else(|| bad(here(r), "invalid CFF offset"))?;
        }
        if n < previous || (i == 0 && n != 1) {
            return Err(bad(here(r), "invalid CFF INDEX offsets"));
        }
        offsets.push(n - 1);
        previous = n;
    }
    let start = here(r);
    let len = *offsets
        .last()
        .ok_or_else(|| bad(here(r), "missing CFF offset"))?;
    r.skip(len)?;
    Ok(offsets
        .windows(2)
        .map(|w| start + w[0]..start + w[1])
        .collect())
}

type CharsetMaps = (BTreeMap<Name, GlyphId>, Option<BTreeMap<u32, GlyphId>>);

fn charset_maps(
    charset: &[u16],
    top: &dict::Dict,
    string_ranges: &[Range<usize>],
    data: &[u8],
    work: &mut Work<'_>,
    at: usize,
) -> Result<CharsetMaps> {
    let mut names = BTreeMap::new();
    let mut cids = top.has(0x0c1e).then(BTreeMap::new);
    for (i, sid) in charset.iter().copied().enumerate() {
        let glyph =
            GlyphId(u32::try_from(i).map_err(|_| crate::read::limit(at, "glyph count overflow"))?);
        if let Some(cids) = &mut cids {
            if f64::from(sid) >= top.number(0x0c22, 8720.0)? {
                return Err(bad(at, "CID outside CFF CIDCount"));
            }
            if cids.insert(u32::from(sid), glyph).is_some() {
                return Err(bad(at, "duplicate CFF CID"));
            }
        } else {
            let name = strings::sid(sid, string_ranges, data, work)?;
            if names.insert(name, glyph).is_some() {
                return Err(bad(at, "duplicate CFF glyph name"));
            }
        }
    }
    Ok((names, cids))
}

impl Cff {
    pub(crate) fn parse(data: &[u8], range: Range<usize>, work: &mut Work<'_>) -> Result<Self> {
        let Header {
            mut top,
            strings: string_ranges,
            global,
            cff2,
        } = header(data, range.clone(), work)?;
        let table = slice(data, range.clone(), "invalid CFF table")?;
        if !cff2 {
            top.validate(false, string_ranges.len(), work)?;
        }
        if top.has(0x0c14) {
            return Err(format_error(
                range.start,
                "synthetic CFF font is not supported",
            ));
        }
        let mut charstrings = table.cursor_at(top.offset(17, None)?)?;
        let glyphs = index(&mut charstrings, work, cff2)?;
        if glyphs.is_empty() {
            return Err(bad(here(&charstrings), "CFF has no glyphs"));
        }
        if !cff2 && top.number(0x0c06, 2.0)? != 2.0 {
            return Err(format_error(
                range.start,
                "CFF CharstringType is not Type 2",
            ));
        }
        let variations = if cff2 && top.has(24) {
            variation::parse(table.cursor_at(top.offset(24, None)?)?, work)?
        } else {
            Vec::new()
        };
        let matrix = top.matrix(Transform::scale(0.001, 0.001))?;
        let cid = top.has(0x0c1e);
        let charset = if cff2 {
            Vec::new()
        } else {
            charset(&table, top.offset(15, Some(0))?, glyphs.len(), work)?
        };
        let (names, cids) = charset_maps(&charset, &top, &string_ranges, data, work, range.start)?;
        let encoding = if cid || cff2 {
            None
        } else {
            Some(strings::encoding(
                &table,
                top.offset(16, Some(0))?,
                &charset,
                &string_ranges,
                data,
                work,
            )?)
        };
        let context = Context {
            cff2,
            variations: &variations,
            strings: string_ranges.len(),
        };
        let (private, select) = dictionaries(&table, &top, data, glyphs.len(), context, work)?;
        let glyph_names = crate::names::indexed(
            glyphs.len(),
            names.iter().map(|(name, glyph)| Ok((*glyph, name))),
            work,
            range.start,
        )?;
        Ok(Self {
            glyphs,
            global,
            private,
            select,
            names,
            glyph_names,
            cids,
            encoding,
            matrix,
            cff2,
            variations,
        })
    }
    pub(crate) fn glyph_count(&self) -> u32 {
        u32::try_from(self.glyphs.len()).unwrap_or(0)
    }
    pub(crate) fn matrix(&self) -> Transform {
        self.matrix
    }
    pub(crate) fn is_cid(&self) -> bool {
        self.cids.is_some()
    }
    pub(crate) fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cids.as_ref()?.get(&cid).copied()
    }
    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        self.glyph_names.get(u32_to_usize(glyph.0))?.as_ref()
    }
    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.names.get(name).copied()
    }
    pub(crate) fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.encoding.as_deref()
    }
    pub(crate) fn outline(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
        Ok(type2::glyph(self, data, glyph, work)?.0)
    }
    pub(crate) fn advance(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Option<f64> {
        if self.cff2 {
            None
        } else {
            type2::glyph(self, data, glyph, work).ok().map(|g| g.1)
        }
    }

    /// The Private DICT that selects `glyph`'s subroutines and widths.
    fn private(&self, glyph: GlyphId) -> Option<&Private> {
        self.private.get(*self.select.get(u32_to_usize(glyph.0))?)
    }

    /// Heap reachable through CFF metadata; INDEX ranges borrow the
    /// original bytes.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.private
            .iter()
            .fold(vector(&self.private), |sum, p| {
                sum.saturating_add(vector(&p.subrs))
            })
            .saturating_add(vector(&self.glyphs))
            .saturating_add(vector(&self.global))
            .saturating_add(vector(&self.select))
            .saturating_add(names(&self.names))
            .saturating_add(crate::names::heap_bytes(&self.glyph_names))
            .saturating_add(self.cids.as_ref().map_or(0, tree))
            .saturating_add(self.encoding.as_deref().map_or(0, encoding))
            .saturating_add(nested(&self.variations))
    }
}

fn private_dict(
    table: &Cursor<'_>,
    top: &dict::Dict,
    context: Context<'_>,
    matrix: Transform,
    work: &mut Work<'_>,
) -> Result<Private> {
    let pair = match top.values(18) {
        Some(pair) => pair,
        None if context.cff2 => &[0.0, 0.0],
        None => return Err(bad(here(table), "missing CFF Private DICT")),
    };
    if pair.len() != 2 {
        return Err(bad(here(table), "invalid CFF Private operands"));
    }
    let size = dict::integer(pair[0], here(table))?;
    let offset = dict::integer(pair[1], here(table))?;
    let mut r = table.cursor_at(offset)?;
    let at = here(&r);
    let bytes = r.take_cursor(size)?;
    let mut private = if context.cff2 {
        dict::parse_private(bytes, work, context.variations)?
    } else {
        dict::parse(bytes, work)?
    };
    if !context.cff2 {
        private.validate(true, 0, work)?;
    }
    private.check_blues(work)?;
    let subrs = if private.has(19) {
        index(
            &mut table.cursor_at(
                offset
                    .checked_add(private.offset(19, None)?)
                    .ok_or_else(|| crate::read::limit(at, "private offset overflow"))?,
            )?,
            work,
            context.cff2,
        )?
    } else {
        Vec::new()
    };
    Ok(Private {
        subrs,
        default_width: private.number(20, 0.0)?,
        nominal_width: private.number(21, 0.0)?,
        matrix,
        vsindex: private.offset(22, Some(0))?,
        random_seed: random_seed(private.number(0x0c13, 0.0)?),
    })
}

fn charset(
    table: &Cursor<'_>,
    offset: usize,
    glyphs: usize,
    work: &mut Work<'_>,
) -> Result<Vec<u16>> {
    if offset == 0 {
        if glyphs > 229 {
            return Err(bad(here(table), "ISOAdobe charset glyph count exceeded"));
        }
        return (0..glyphs)
            .map(|i| {
                u16::try_from(i).map_err(|_| crate::read::limit(here(table), "charset overflow"))
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
            .ok_or_else(|| bad(here(table), "predefined CFF charset glyph count exceeded"));
    }
    let mut r = table.cursor_at(offset)?;
    let format = r.u8()?;
    work.entries(glyphs, Position::Buffer(here(&r)))?;
    let mut result = Vec::with_capacity(glyphs);
    result.push(0);
    while result.len() < glyphs {
        let first = r.u16()?;
        let left = match format {
            0 => 0,
            1 => u16::from(r.u8()?),
            2 => r.u16()?,
            _ => return Err(bad(here(&r), "invalid CFF charset format")),
        };
        if usize::from(left) + 1 > glyphs - result.len() || first.checked_add(left).is_none() {
            return Err(bad(here(&r), "CFF charset range exceeds glyph count"));
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
fn header(data: &[u8], range: Range<usize>, work: &mut Work<'_>) -> Result<Header> {
    let mut r = slice(data, range, "invalid CFF table")?;
    let major = r.u8()?;
    if !matches!(major, 1 | 2) {
        return Err(bad(here(&r), "invalid CFF version"));
    }
    let cff2 = major == 2;
    r.u8()?;
    let header = usize::from(r.u8()?);
    let (top, strings, global) = if cff2 {
        let size = usize::from(r.u16()?);
        if header < 5 {
            return Err(bad(here(&r), "invalid CFF2 header"));
        }
        r = r.cursor_at(header)?;
        let top = dict::parse_cff2(r.take_cursor(size)?, work)?;
        (top, Vec::new(), index(&mut r, work, true)?)
    } else {
        let offset_size = r.u8()?;
        if header < 4 || !(1..=4).contains(&offset_size) {
            return Err(bad(here(&r), "invalid CFF header"));
        }
        r = r.cursor_at(header)?;
        if index(&mut r, work, false)?.len() != 1 {
            return Err(bad(here(&r), "embedded CFF must contain one font"));
        }
        let tops = index(&mut r, work, false)?;
        let top = tops
            .first()
            .filter(|_| tops.len() == 1)
            .ok_or_else(|| bad(here(&r), "CFF Top DICT count mismatch"))?;
        let top = dict::parse(slice(data, top.clone(), "CFF Top DICT outside font")?, work)?;
        let strings = index(&mut r, work, false)?;
        let global = index(&mut r, work, false)?;
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
    table: &Cursor<'_>,
    top: &dict::Dict,
    data: &[u8],
    glyphs: usize,
    context: Context<'_>,
    work: &mut Work<'_>,
) -> Result<(Vec<Private>, Vec<usize>)> {
    let cff2 = context.cff2;
    let (private, select) = if top.has(0x0c1e) || cff2 && top.has(0x0c24) {
        let mut array = table.cursor_at(top.offset(0x0c24, None)?)?;
        let fonts = index(&mut array, work, cff2)?;
        if fonts.is_empty() || fonts.len() > if cff2 { 65536 } else { 256 } {
            return Err(bad(here(&array), "invalid CFF FDArray count"));
        }
        let mut private = Vec::with_capacity(fonts.len());
        for font in fonts {
            work.work(font.len(), Position::Buffer(font.start))?;
            let bytes = slice(data, font.clone(), "CFF Font DICT outside font")?;
            let mut dict = if cff2 {
                dict::parse_cff2(bytes, work)?
            } else {
                dict::parse(bytes, work)?
            };
            if !cff2 {
                dict.validate(false, context.strings, work)?;
            }
            if cff2 && dict.has(0x0c07) {
                // Secondary outline readers disagree for this unreviewed
                // CFF2 combination. Keep it explicit until primary review.
                return Err(pagura_core::Error::unsupported(
                    crate::features::FORMAT,
                    "CFF2 Font DICT matrices are not supported",
                )
                .at(Position::Buffer(font.start)));
            }
            let parsed = private_dict(
                table,
                &dict,
                context,
                dict.matrix(Transform::IDENTITY)?,
                work,
            )?;
            private.push(parsed);
        }
        let select = if cff2 && !top.has(0x0c25) && private.len() == 1 {
            vec![0; glyphs]
        } else {
            select::parse(
                table.cursor_at(top.offset(0x0c25, None)?)?,
                glyphs,
                private.len(),
                work,
                cff2,
            )?
        };
        (private, select)
    } else {
        (
            vec![private_dict(
                table,
                top,
                context,
                Transform::IDENTITY,
                work,
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

fn format_error(at: usize, message: &'static str) -> pagura_core::Error {
    pagura_core::Error::unsupported(crate::features::FORMAT, message).at(Position::Buffer(at))
}
