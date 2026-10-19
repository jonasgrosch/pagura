//! sfnt directory and horizontal metrics (ISO 32000-2, 9.9).
use crate::memory::{names, vector};
use crate::read::{bad, cap, here, slice};
use crate::{CmapSubtable, GlyphId};
use crate::{recovery::repair, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostics, Limits, Name, Position, Result};
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
    locations: Vec<Option<usize>>,
    advances: Option<Vec<u16>>,
    glyph_count: u32,
    units: u16,
    cmaps: Vec<cmap::Cmap>,
    names: names::PostNames,
}

impl Sfnt {
    pub(crate) fn parse(
        data: &[u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let (signature, tables, clipped_post) = directory(data, limits, diagnostics)?;
        let table = |tag: &[u8; 4]| -> Result<Cursor<'_>> {
            let range = tables
                .get(tag)
                .ok_or_else(|| bad(0, "missing required sfnt table"))?;
            slice(data, range.clone(), "sfnt table outside font")
        };
        let (units, location_format, glyph_count, advances, cff) = if signature == 0x4f54_544f {
            let cff = cff_tables(data, &tables, limits, diagnostics)?;
            let count = usize::try_from(cff.glyph_count())
                .map_err(|_| crate::read::limit(0, "glyph count overflow"))?;
            // ISO 32000-2, 9.9.1 Table 124: CFF OpenType needs CFF and cmap,
            // without imposing glyf's head/hhea/hmtx/loca/maxp requirements.
            table(b"cmap")?;
            if tables.contains_key(b"maxp") {
                let mut maxp = table(b"maxp")?;
                if maxp.u32()? != 0x0000_5000 || usize::from(maxp.u16()?) != count {
                    return Err(bad(here(&maxp), "CFF and maxp glyph counts disagree"));
                }
            }
            let units = if tables.contains_key(b"head") {
                header(table(b"head")?)?.0
            } else {
                1000
            };
            let advances = if tables.contains_key(b"hhea") || tables.contains_key(b"hmtx") {
                Some(metrics(
                    table(b"hhea")?,
                    table(b"hmtx")?,
                    count,
                    diagnostics,
                )?)
            } else {
                None
            };
            (units, 0, count, advances, Some(cff))
        } else {
            let (units, location_format) = header(table(b"head")?)?;
            let glyph_count = maxp_count(table(b"maxp")?, signature, limits)?;
            let advances = Some(metrics(
                table(b"hhea")?,
                table(b"hmtx")?,
                glyph_count,
                diagnostics,
            )?);
            (units, location_format, glyph_count, advances, None)
        };
        let (glyf, locations) = if cff.is_none() {
            let glyf = tables
                .get(b"glyf")
                .ok_or_else(|| bad(0, "missing glyf table"))?
                .clone();
            let locations = locations(
                table(b"loca")?,
                glyph_count,
                location_format,
                glyf.len(),
                diagnostics,
            )?;
            (glyf, locations)
        } else {
            (0..0, Vec::new())
        };
        let cmaps = if tables.contains_key(b"cmap") {
            match cmap::parse(table(b"cmap")?, glyph_count, limits, diagnostics) {
                Ok(cmaps) => cmaps,
                Err(error) if crate::recovery::glyph_error(&error) => {
                    repair(
                        diagnostics,
                        repairs::CMAP_DATA,
                        error.message,
                        error.position,
                    )?;
                    Vec::new()
                }
                Err(error) => return Err(error),
            }
        } else {
            Vec::new()
        };
        let names = if tables.contains_key(b"post") {
            match names::parse(table(b"post")?, glyph_count, limits) {
                Ok(names) => names,
                Err(error) if clipped_post && crate::recovery::glyph_error(&error) => {
                    names::PostNames::default()
                }
                Err(error) => return Err(error),
            }
        } else {
            names::PostNames::default()
        };
        Ok(Self {
            glyf,
            cff,
            locations,
            advances,
            glyph_count: u32::try_from(glyph_count)
                .map_err(|_| crate::read::limit(0, "glyph count overflow"))?,
            units,
            cmaps,
            names,
        })
    }
    pub(crate) fn is_cid(&self) -> bool {
        self.cff.as_ref().is_some_and(crate::cff::Cff::is_cid)
    }
    pub(crate) fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cff.as_ref()?.cid(cid)
    }
    pub(crate) fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.cff.as_ref()?.encoding()
    }
    pub(crate) fn glyph_count(&self) -> u32 {
        self.glyph_count
    }
    pub(crate) fn matrix(&self) -> Transform {
        self.cff.as_ref().map_or_else(
            || {
                let scale = 1.0 / f64::from(self.units);
                Transform::scale(scale, scale)
            },
            crate::cff::Cff::matrix,
        )
    }
    pub(crate) fn advance(&self, data: &[u8], glyph: GlyphId, limits: &Limits) -> Option<f64> {
        if let Some(advances) = &self.advances {
            advances
                .get(usize::try_from(glyph.0).ok()?)
                .copied()
                .map(f64::from)
        } else {
            self.cff.as_ref()?.advance(data, glyph, limits)
        }
    }
    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        self.cff
            .as_ref()
            .and_then(|cff| cff.glyph_name(glyph))
            .or_else(|| {
                self.names
                    .by_glyph
                    .get(usize::try_from(glyph.0).ok()?)?
                    .as_ref()
            })
    }
    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.cff
            .as_ref()
            .and_then(|cff| cff.name(name))
            .or_else(|| self.names.by_name.get(name).copied())
    }
    pub(crate) fn has_cmap(&self, subtable: CmapSubtable) -> bool {
        self.cmaps.iter().any(|c| c.selection == subtable)
    }
    pub(crate) fn code(&self, data: &[u8], subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
        self.cmaps
            .iter()
            .filter(|c| c.selection == subtable)
            .find_map(|c| c.lookup(data, code))
            .filter(|g| g.0 < self.glyph_count())
    }
    pub(crate) fn outline(
        &self,
        data: &[u8],
        glyph: GlyphId,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Path> {
        if let Some(cff) = &self.cff {
            cff.outline(data, glyph, limits, diagnostics)
        } else {
            glyf::outline(self, data, glyph, limits, diagnostics)
        }
    }

    /// Heap beyond the table bytes, which the program's input owns.
    pub(crate) fn heap_bytes(&self) -> usize {
        vector(&self.locations)
            .saturating_add(self.advances.as_ref().map_or(0, vector))
            .saturating_add(vector(&self.cmaps))
            .saturating_add(names(&self.names.by_name))
            .saturating_add(crate::names::heap_bytes(&self.names.by_glyph))
            .saturating_add(self.cff.as_ref().map_or(0, crate::cff::Cff::heap_bytes))
    }
}

mod directory;
use directory::directory;

fn locations(
    mut r: Cursor<'_>,
    count: usize,
    format: i16,
    glyph_bytes: usize,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Option<usize>>> {
    let mut result = Vec::with_capacity(count + 1);
    let mut previous = 0;
    let word = if format == 0 { 2 } else { 4 };
    for _ in 0..=count {
        if r.remaining() < word {
            repair(
                diagnostics,
                repairs::TABLE_BOUNDS,
                "truncated glyph-location table",
                Position::Buffer(here(&r)),
            )?;
            result.resize(count + 1, None);
            break;
        }
        let offset = if format == 0 {
            usize::from(r.u16()?) * 2
        } else {
            u32_to_usize(r.u32()?)
        };
        if offset < previous {
            return Err(bad(here(&r), "invalid glyph location"));
        }
        if offset > glyph_bytes {
            repair(
                diagnostics,
                repairs::TABLE_BOUNDS,
                "glyph location exceeds available outlines",
                Position::Buffer(here(&r) - word),
            )?;
        }
        result.push(Some(offset));
        previous = offset;
    }
    Ok(result)
}

fn metrics(
    mut hhea: Cursor<'_>,
    mut hmtx: Cursor<'_>,
    glyph_count: usize,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u16>> {
    if hhea.u32()? != 0x0001_0000 {
        return Err(bad(here(&hhea), "invalid hhea version"));
    }
    hhea.skip(28)?;
    if hhea.i16()? != 0 {
        return Err(bad(here(&hhea), "invalid horizontal metric data format"));
    }
    let metrics = usize::from(hhea.u16()?);
    if metrics == 0 || metrics > glyph_count {
        return Err(bad(here(&hhea), "invalid horizontal metric count"));
    }
    // Only advances are consumed by this outline API. Missing side bearings
    // may be ignored if every explicitly stored advance remains available.
    let minimum = metrics * 4 - 2;
    if hmtx.remaining() < minimum {
        return Err(bad(here(&hmtx), "missing required horizontal advances"));
    }
    if hmtx.remaining() < metrics * 4 + (glyph_count - metrics) * 2 {
        repair(
            diagnostics,
            repairs::TABLE_BOUNDS,
            "truncated unused horizontal side bearings",
            Position::Buffer(here(&hmtx) + hmtx.remaining()),
        )?;
    }
    let mut advances = Vec::with_capacity(glyph_count);
    let mut last = 0;
    for _ in 0..metrics {
        last = hmtx.u16()?;
        if hmtx.remaining() >= 2 {
            hmtx.skip(2)?;
        }
        advances.push(last);
    }
    advances.resize(glyph_count, last);
    Ok(advances)
}

fn header(mut head: Cursor<'_>) -> Result<(u16, i16)> {
    if head.u32()? != 0x0001_0000 {
        return Err(bad(here(&head), "invalid head version"));
    }
    head.skip(8)?;
    if head.u32()? != 0x5f0f_3cf5 {
        return Err(bad(here(&head), "invalid head magic"));
    }
    head.skip(2)?;
    let units = head.u16()?;
    if !(16..=16384).contains(&units) {
        return Err(bad(here(&head), "invalid unitsPerEm"));
    }
    head.skip(30)?;
    let location_format = head.i16()?;
    if !matches!(location_format, 0 | 1) || head.i16()? != 0 {
        return Err(bad(here(&head), "invalid glyph data format"));
    }
    Ok((units, location_format))
}

fn cff_tables(
    data: &[u8],
    tables: &Tables,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<crate::cff::Cff> {
    let range = tables
        .get(b"CFF ")
        .or_else(|| tables.get(b"CFF2"))
        .ok_or_else(|| bad(0, "missing OpenType CFF table"))?;
    crate::cff::Cff::parse(data, range.clone(), limits, diagnostics)
}

fn maxp_count(mut maxp: Cursor<'_>, signature: u32, limits: &Limits) -> Result<usize> {
    let maxp_version = maxp.u32()?;
    if maxp_version
        != if signature == 0x0001_0000 {
            0x0001_0000
        } else {
            0x0000_5000
        }
    {
        return Err(bad(here(&maxp), "invalid maxp version for glyf"));
    }
    let glyph_count = usize::from(maxp.u16()?);
    if signature == 0x0001_0000 {
        maxp.skip(26)?;
    }
    if glyph_count == 0 {
        return Err(bad(here(&maxp), "font has no glyphs"));
    }
    cap(
        glyph_count,
        limits.objects.min(limits.value_nodes),
        here(&maxp),
    )?;
    Ok(glyph_count)
}
