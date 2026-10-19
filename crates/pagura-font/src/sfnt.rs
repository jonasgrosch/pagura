//! sfnt containers: table directory, header, horizontal metrics and glyph
//! locations (9.9; OpenType).
use crate::budget::Work;
use crate::cff::Cff;
use crate::memory::{names, vector};
use crate::read::{self, here, syntax};
use crate::recovery::glyph_error;
use crate::{CmapSubtable, GlyphId, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};
use pagura_raster::{Path, Transform};
use std::collections::BTreeMap;
use std::ops::{Range, RangeInclusive};

mod cmap;
mod directory;
mod glyf;
mod names;

use directory::{Directory, Flavor};

type Tables = BTreeMap<[u8; 4], Range<usize>>;

/// The offset table at the start of the program, which lists every table.
const DIRECTORY: Position = Position::Buffer(0);
/// Version 1.0 of head, hhea and maxp; maxp 0.5 holds only a glyph count.
const VERSION_1_0: u32 = 0x0001_0000;
const MAXP_VERSION_0_5: u32 = 0x0000_5000;
/// The head table's magicNumber and its unitsPerEm range.
const HEAD_MAGIC: u32 = 0x5f0f_3cf5;
const UNITS_PER_EM: RangeInclusive<u16> = 16..=16384;
/// The em size assumed for CFF outlines without a head table.
const CFF_UNITS_PER_EM: u16 = 1000;
/// head.indexToLocFormat values.
const SHORT_OFFSETS: i16 = 0;
const LONG_OFFSETS: i16 = 1;

/// The width of each loca entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocaFormat {
    /// 16-bit offsets, in units of two bytes.
    Short,
    /// 32-bit byte offsets.
    Long,
}

/// The head table fields this reader uses.
struct Header {
    units: u16,
    loca: LocaFormat,
}

#[derive(Debug)]
enum Outlines {
    Glyf(glyf::Glyf),
    Cff(Box<Cff>),
}

#[derive(Debug)]
pub(crate) struct Sfnt {
    outlines: Outlines,
    advances: Option<Vec<u16>>,
    glyph_count: u32,
    units: u16,
    cmaps: Vec<cmap::Cmap>,
    names: names::PostNames,
}

/// One table cursor, or `None` if the font lacks the table.
struct Table<'a> {
    data: &'a [u8],
    tables: &'a Tables,
}

impl<'a> Table<'a> {
    fn get(&self, tag: [u8; 4]) -> Option<Result<Cursor<'a>>> {
        let range = self.tables.get(&tag)?.clone();
        Some(read::slice(self.data, range, "sfnt table outside font"))
    }

    fn required(&self, tag: [u8; 4]) -> Result<Cursor<'a>> {
        self.get(tag)
            .unwrap_or_else(|| Err(syntax(DIRECTORY, "missing required sfnt table")))
    }

    fn has(&self, tag: [u8; 4]) -> bool {
        self.tables.contains_key(&tag)
    }
}

impl Sfnt {
    pub(crate) fn parse(data: &[u8], work: &mut Work<'_>) -> Result<Self> {
        let Directory {
            flavor,
            tables,
            clipped_post,
        } = directory::parse(data, work)?;
        let table = Table {
            data,
            tables: &tables,
        };
        let Container {
            units,
            glyph_count,
            advances,
            outlines,
        } = match flavor {
            Flavor::Cff => cff_container(&table, work)?,
            Flavor::TrueType => glyf_container(&table, work)?,
        };
        let cmaps = table.get(*b"cmap").map_or(Ok(Vec::new()), |cmap| {
            cmap.and_then(|cmap| cmap::parse(cmap, glyph_count, work))
        });
        let cmaps = match cmaps {
            Ok(cmaps) => cmaps,
            Err(error) if glyph_error(&error) => {
                work.repair(repairs::CMAP_DATA, error.message, error.position)?;
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        let names = table
            .get(*b"post")
            .map_or(Ok(names::PostNames::default()), |post| {
                post.and_then(|post| names::parse(post, glyph_count, work))
            });
        let names = match names {
            Ok(names) => names,
            // A post table clamped to the available data loses only its names.
            Err(error) if clipped_post && glyph_error(&error) => names::PostNames::default(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            outlines,
            advances,
            glyph_count: read::glyph_id(glyph_count, DIRECTORY)?.0,
            units,
            cmaps,
            names,
        })
    }

    fn cff(&self) -> Option<&Cff> {
        match &self.outlines {
            Outlines::Cff(cff) => Some(cff),
            Outlines::Glyf(_) => None,
        }
    }

    pub(crate) fn is_cid(&self) -> bool {
        self.cff().is_some_and(Cff::is_cid)
    }

    pub(crate) fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cff()?.cid(cid)
    }

    pub(crate) fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.cff()?.encoding()
    }

    pub(crate) const fn glyph_count(&self) -> u32 {
        self.glyph_count
    }

    pub(crate) fn matrix(&self) -> Transform {
        self.cff().map_or_else(
            || {
                let scale = 1.0 / f64::from(self.units);
                Transform::scale(scale, scale)
            },
            Cff::matrix,
        )
    }

    /// The `hmtx` advance, else the CFF charstring width.
    pub(crate) fn advance(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Option<f64> {
        match (&self.advances, &self.outlines) {
            (Some(advances), _) => advances.get(u32_to_usize(glyph.0)).copied().map(f64::from),
            (None, Outlines::Cff(cff)) => cff.advance(data, glyph, work),
            (None, Outlines::Glyf(_)) => None,
        }
    }

    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.cff()
            .and_then(|cff| cff.name(name))
            .or_else(|| self.names.by_name.get(name).copied())
    }

    /// The CFF charset name, else the `post` name.
    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        self.cff()
            .and_then(|cff| cff.glyph_name(glyph))
            .or_else(|| crate::names::get(&self.names.by_glyph, glyph))
    }

    pub(crate) fn has_cmap(&self, subtable: CmapSubtable) -> bool {
        self.cmaps.iter().any(|c| c.selection() == subtable)
    }

    pub(crate) fn code(&self, data: &[u8], subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
        self.cmaps
            .iter()
            .filter(|c| c.selection() == subtable)
            .find_map(|c| c.lookup(data, code))
            .filter(|g| g.0 < self.glyph_count())
    }

    pub(crate) fn outline(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
        match &self.outlines {
            Outlines::Cff(cff) => cff.outline(data, glyph, work),
            Outlines::Glyf(glyf) => glyf.outline(data, glyph, work),
        }
    }

    /// Heap beyond the table bytes, which the program's input owns.
    pub(crate) fn heap_bytes(&self) -> usize {
        let outlines = match &self.outlines {
            Outlines::Glyf(glyf) => glyf.heap_bytes(),
            Outlines::Cff(cff) => cff.heap_bytes(),
        };
        outlines
            .saturating_add(self.advances.as_ref().map_or(0, vector))
            .saturating_add(vector(&self.cmaps))
            .saturating_add(names(&self.names.by_name))
            .saturating_add(crate::names::heap_bytes(&self.names.by_glyph))
    }
}

/// The outlines of a container and the metrics that come with them.
struct Container {
    units: u16,
    glyph_count: usize,
    advances: Option<Vec<u16>>,
    outlines: Outlines,
}

fn cff_container(table: &Table<'_>, work: &mut Work<'_>) -> Result<Container> {
    let cff = cff_table(table.data, table.tables, work)?;
    let glyph_count = u32_to_usize(cff.glyph_count());
    // 9.9.1 Table 124: CFF OpenType needs CFF and cmap, without imposing
    // glyf's head/hhea/hmtx/loca/maxp requirements.
    table.required(*b"cmap")?;
    if let Some(maxp) = table.get(*b"maxp") {
        let mut maxp = maxp?;
        if maxp.u32()? != MAXP_VERSION_0_5 || usize::from(maxp.u16()?) != glyph_count {
            return Err(maxp.error("CFF and maxp glyph counts disagree"));
        }
    }
    let units = if let Some(head) = table.get(*b"head") {
        header(head?)?.units
    } else {
        CFF_UNITS_PER_EM
    };
    let advances = if table.has(*b"hhea") || table.has(*b"hmtx") {
        Some(metrics(
            table.required(*b"hhea")?,
            table.required(*b"hmtx")?,
            glyph_count,
            work,
        )?)
    } else {
        None
    };
    Ok(Container {
        units,
        glyph_count,
        advances,
        outlines: Outlines::Cff(Box::new(cff)),
    })
}

fn glyf_container(table: &Table<'_>, work: &mut Work<'_>) -> Result<Container> {
    let Header { units, loca } = header(table.required(*b"head")?)?;
    let glyph_count = glyph_count(table.required(*b"maxp")?, work)?;
    let advances = metrics(
        table.required(*b"hhea")?,
        table.required(*b"hmtx")?,
        glyph_count,
        work,
    )?;
    let glyf = table
        .tables
        .get(b"glyf")
        .ok_or_else(|| syntax(DIRECTORY, "missing glyf table"))?
        .clone();
    let locations = locations(
        table.required(*b"loca")?,
        glyph_count,
        loca,
        glyf.len(),
        work,
    )?;
    Ok(Container {
        units,
        glyph_count,
        advances: Some(advances),
        outlines: Outlines::Glyf(glyf::Glyf::new(glyf, locations)),
    })
}

fn cff_table(data: &[u8], tables: &Tables, work: &mut Work<'_>) -> Result<Cff> {
    let range = tables
        .get(b"CFF ")
        .or_else(|| tables.get(b"CFF2"))
        .ok_or_else(|| syntax(DIRECTORY, "missing OpenType CFF table"))?;
    Cff::parse(data, range.clone(), work)
}

fn locations(
    mut r: Cursor<'_>,
    count: usize,
    format: LocaFormat,
    glyph_bytes: usize,
    work: &mut Work<'_>,
) -> Result<Vec<Option<usize>>> {
    let mut result = Vec::with_capacity(count + 1);
    let mut previous = 0;
    let word = match format {
        LocaFormat::Short => 2,
        LocaFormat::Long => 4,
    };
    for _ in 0..=count {
        if r.remaining() < word {
            work.repair(
                repairs::TABLE_BOUNDS,
                "truncated glyph-location table",
                r.position(),
            )?;
            result.resize(count + 1, None);
            break;
        }
        let at = r.position();
        let offset = match format {
            LocaFormat::Short => usize::from(r.u16()?) * 2,
            LocaFormat::Long => u32_to_usize(r.u32()?),
        };
        if offset < previous {
            return Err(r.error("invalid glyph location"));
        }
        if offset > glyph_bytes {
            work.repair(
                repairs::TABLE_BOUNDS,
                "glyph location exceeds available outlines",
                at,
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
    work: &mut Work<'_>,
) -> Result<Vec<u16>> {
    if hhea.u32()? != VERSION_1_0 {
        return Err(hhea.error("invalid hhea version"));
    }
    hhea.skip(28)?; // ascender .. caretOffset and the reserved fields
    if hhea.i16()? != 0 {
        return Err(hhea.error("invalid horizontal metric data format"));
    }
    let metrics = usize::from(hhea.u16()?);
    if metrics == 0 || metrics > glyph_count {
        return Err(hhea.error("invalid horizontal metric count"));
    }
    // Only advances are consumed by this outline API. Missing side bearings
    // may be ignored if every explicitly stored advance remains available.
    if hmtx.remaining() < metrics * 4 - 2 {
        return Err(hmtx.error("missing required horizontal advances"));
    }
    if hmtx.remaining() < metrics * 4 + (glyph_count - metrics) * 2 {
        work.repair(
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
            hmtx.skip(2)?; // left side bearing
        }
        advances.push(last);
    }
    advances.resize(glyph_count, last);
    Ok(advances)
}

fn header(mut head: Cursor<'_>) -> Result<Header> {
    if head.u32()? != VERSION_1_0 {
        return Err(head.error("invalid head version"));
    }
    head.skip(8)?; // fontRevision, checksumAdjustment
    if head.u32()? != HEAD_MAGIC {
        return Err(head.error("invalid head magic"));
    }
    head.skip(2)?; // flags
    let units = head.u16()?;
    if !UNITS_PER_EM.contains(&units) {
        return Err(head.error("invalid unitsPerEm"));
    }
    head.skip(30)?; // created .. fontDirectionHint
    let loca = match head.i16()? {
        SHORT_OFFSETS => LocaFormat::Short,
        LONG_OFFSETS => LocaFormat::Long,
        _ => return Err(head.error("invalid glyph data format")),
    };
    if head.i16()? != 0 {
        return Err(head.error("invalid glyph data format"));
    }
    Ok(Header { units, loca })
}

/// The glyph count of a TrueType-outline font's maxp table (version 1.0).
fn glyph_count(mut maxp: Cursor<'_>, work: &mut Work<'_>) -> Result<usize> {
    if maxp.u32()? != VERSION_1_0 {
        return Err(maxp.error("invalid maxp version for glyf"));
    }
    let count = usize::from(maxp.u16()?);
    maxp.skip(26)?; // the version 1.0 fields after numGlyphs
    if count == 0 {
        return Err(maxp.error("font has no glyphs"));
    }
    work.entries(count, maxp.position())?;
    Ok(count)
}
