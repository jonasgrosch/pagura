//! sfnt directory and horizontal metrics (ISO 32000-2, 9.9).
use crate::read::{Read, bad, cap};
use crate::{CmapSubtable, GlyphId};
use pagura_core::{Limits, Name, Result};
use pagura_raster::{Path, Transform};
use std::collections::BTreeMap;
use std::ops::Range;

mod cmap;
mod glyf;
mod names;

type Tables = BTreeMap<[u8; 4], Range<usize>>;

#[derive(Debug)]
pub(crate) struct Sfnt {
    glyf: Range<usize>,
    cff: Option<crate::cff::Cff>,
    locations: Vec<usize>,
    advances: Option<Vec<u16>>,
    glyph_count: u32,
    units: u16,
    cmaps: Vec<cmap::Cmap>,
    names: BTreeMap<Name, GlyphId>,
}

impl Sfnt {
    pub fn parse(data: &[u8], limits: &Limits) -> Result<Self> {
        let (signature, tables) = directory(data, limits)?;
        let table = |tag: &[u8; 4]| -> Result<Read<'_>> {
            let range = tables
                .get(tag)
                .ok_or_else(|| bad(0, "missing required sfnt table"))?;
            Ok(Read::new(&data[range.clone()], range.start))
        };
        let (units, location_format, glyph_count, advances, cff) = if signature == 0x4f54_544f {
            let cff = cff_tables(data, &tables, limits)?;
            let count =
                usize::try_from(cff.glyph_count()).map_err(|_| bad(0, "glyph count overflow"))?;
            // ISO 32000-2, 9.9.1 Table 124: CFF OpenType needs CFF and cmap,
            // without imposing glyf's head/hhea/hmtx/loca/maxp requirements.
            table(b"cmap")?;
            if tables.contains_key(b"maxp") {
                let mut maxp = table(b"maxp")?;
                if maxp.u32()? != 0x0000_5000 || usize::from(maxp.u16()?) != count {
                    return Err(bad(maxp.at(), "CFF and maxp glyph counts disagree"));
                }
            }
            let units = if tables.contains_key(b"head") {
                header(table(b"head")?)?.0
            } else {
                1000
            };
            let advances = if tables.contains_key(b"hhea") || tables.contains_key(b"hmtx") {
                Some(metrics(table(b"hhea")?, table(b"hmtx")?, count)?)
            } else {
                None
            };
            (units, 0, count, advances, Some(cff))
        } else {
            let (units, location_format) = header(table(b"head")?)?;
            let glyph_count = maxp_count(table(b"maxp")?, signature, limits)?;
            let advances = Some(metrics(table(b"hhea")?, table(b"hmtx")?, glyph_count)?);
            (units, location_format, glyph_count, advances, None)
        };
        let (glyf, locations) = if cff.is_none() {
            let glyf = tables
                .get(b"glyf")
                .ok_or_else(|| bad(0, "missing glyf table"))?
                .clone();
            let mut loca = table(b"loca")?;
            let mut locations = Vec::with_capacity(glyph_count + 1);
            let mut previous = 0;
            for _ in 0..=glyph_count {
                let offset = if location_format == 0 {
                    usize::from(loca.u16()?) * 2
                } else {
                    loca.offset()?
                };
                if offset < previous || offset > glyf.len() {
                    return Err(bad(loca.at(), "invalid glyph location"));
                }
                locations.push(offset);
                previous = offset;
            }
            (glyf, locations)
        } else {
            (0..0, Vec::new())
        };
        let cmaps = if tables.contains_key(b"cmap") {
            cmap::parse(table(b"cmap")?, glyph_count, limits)?
        } else {
            Vec::new()
        };
        let names = if tables.contains_key(b"post") {
            names::parse(table(b"post")?, glyph_count, limits)?
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            glyf,
            cff,
            locations,
            advances,
            glyph_count: u32::try_from(glyph_count).map_err(|_| bad(0, "glyph count overflow"))?,
            units,
            cmaps,
            names,
        })
    }
    pub fn is_cid(&self) -> bool {
        self.cff.as_ref().is_some_and(crate::cff::Cff::is_cid)
    }
    pub fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cff.as_ref()?.cid(cid)
    }
    pub fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.cff.as_ref()?.encoding()
    }
    pub fn glyph_count(&self) -> u32 {
        self.glyph_count
    }
    pub fn matrix(&self) -> Transform {
        self.cff.as_ref().map_or_else(
            || {
                let scale = 1.0 / f64::from(self.units);
                Transform::scale(scale, scale)
            },
            crate::cff::Cff::matrix,
        )
    }
    pub fn advance(&self, data: &[u8], glyph: GlyphId, limits: &Limits) -> Option<f64> {
        if let Some(advances) = &self.advances {
            advances
                .get(usize::try_from(glyph.0).ok()?)
                .copied()
                .map(f64::from)
        } else {
            self.cff.as_ref()?.advance(data, glyph, limits)
        }
    }
    pub fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.cff
            .as_ref()
            .and_then(|cff| cff.name(name))
            .or_else(|| self.names.get(name).copied())
    }
    pub fn has_cmap(&self, subtable: CmapSubtable) -> bool {
        self.cmaps.iter().any(|c| c.selection == subtable)
    }
    pub fn code(&self, data: &[u8], subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
        self.cmaps
            .iter()
            .filter(|c| c.selection == subtable)
            .find_map(|c| c.lookup(data, code))
            .filter(|g| g.0 < self.glyph_count())
    }
    pub fn outline(&self, data: &[u8], glyph: GlyphId, limits: &Limits) -> Result<Path> {
        if let Some(cff) = &self.cff {
            cff.outline(data, glyph, limits)
        } else {
            glyf::outline(self, data, glyph, limits)
        }
    }
}

fn directory(data: &[u8], limits: &Limits) -> Result<(u32, Tables)> {
    let mut r = Read::new(data, 0);
    let signature = r.u32()?;
    if !matches!(signature, 0x0001_0000 | 0x4f54_544f) {
        return Err(bad(0, "unsupported sfnt version"));
    }
    let count = usize::from(r.u16()?);
    cap(
        count,
        limits
            .objects
            .min(limits.operations)
            .min(limits.value_nodes),
        4,
    )?;
    r.skip(6)?;
    let mut tables = BTreeMap::new();
    for _ in 0..count {
        let tag: [u8; 4] = r
            .take(4)?
            .try_into()
            .map_err(|_| bad(r.at(), "invalid table tag"))?;
        r.skip(4)?;
        let start = r.offset()?;
        let len = r.offset()?;
        cap(len, limits.decoded_bytes, start)?;
        if len != 0 && (start < 12 + 16 * count || start % 4 != 0) {
            return Err(bad(start, "invalid sfnt table offset"));
        }
        let end = start
            .checked_add(len)
            .ok_or_else(|| bad(start, "table size overflow"))?;
        data.get(start..end)
            .ok_or_else(|| bad(start, "truncated sfnt table"))?;
        if tables.insert(tag, start..end).is_some() {
            return Err(bad(r.at(), "duplicate sfnt table"));
        }
    }
    let mut ranges: Vec<_> = tables.values().filter(|r| !r.is_empty()).collect();
    ranges.sort_by_key(|r| r.start);
    if ranges.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(bad(0, "overlapping sfnt tables"));
    }
    Ok((signature, tables))
}

fn metrics(mut hhea: Read<'_>, mut hmtx: Read<'_>, glyph_count: usize) -> Result<Vec<u16>> {
    if hhea.u32()? != 0x0001_0000 {
        return Err(bad(hhea.at(), "invalid hhea version"));
    }
    hhea.skip(28)?;
    if hhea.i16()? != 0 {
        return Err(bad(hhea.at(), "invalid horizontal metric data format"));
    }
    let metrics = usize::from(hhea.u16()?);
    if metrics == 0 || metrics > glyph_count {
        return Err(bad(hhea.at(), "invalid horizontal metric count"));
    }
    let mut advances = Vec::with_capacity(glyph_count);
    let mut last = 0;
    for _ in 0..metrics {
        last = hmtx.u16()?;
        hmtx.skip(2)?;
        advances.push(last);
    }
    for _ in metrics..glyph_count {
        hmtx.skip(2)?;
        advances.push(last);
    }
    Ok(advances)
}

fn header(mut head: Read<'_>) -> Result<(u16, i16)> {
    if head.u32()? != 0x0001_0000 {
        return Err(bad(head.at(), "invalid head version"));
    }
    head.skip(8)?;
    if head.u32()? != 0x5f0f_3cf5 {
        return Err(bad(head.at(), "invalid head magic"));
    }
    head.skip(2)?;
    let units = head.u16()?;
    if !(16..=16384).contains(&units) {
        return Err(bad(head.at(), "invalid unitsPerEm"));
    }
    head.skip(30)?;
    let location_format = head.i16()?;
    if !matches!(location_format, 0 | 1) || head.i16()? != 0 {
        return Err(bad(head.at(), "invalid glyph data format"));
    }
    Ok((units, location_format))
}

fn cff_tables(data: &[u8], tables: &Tables, limits: &Limits) -> Result<crate::cff::Cff> {
    let range = tables
        .get(b"CFF ")
        .or_else(|| tables.get(b"CFF2"))
        .ok_or_else(|| bad(0, "missing OpenType CFF table"))?;
    crate::cff::Cff::parse(data, range.clone(), limits)
}

fn maxp_count(mut maxp: Read<'_>, signature: u32, limits: &Limits) -> Result<usize> {
    let maxp_version = maxp.u32()?;
    if maxp_version
        != if signature == 0x0001_0000 {
            0x0001_0000
        } else {
            0x0000_5000
        }
    {
        return Err(bad(maxp.at(), "invalid maxp version for glyf"));
    }
    let glyph_count = usize::from(maxp.u16()?);
    if signature == 0x0001_0000 {
        maxp.skip(26)?;
    }
    if glyph_count == 0 {
        return Err(bad(maxp.at(), "font has no glyphs"));
    }
    cap(
        glyph_count,
        limits.objects.min(limits.value_nodes),
        maxp.at(),
    )?;
    Ok(glyph_count)
}
