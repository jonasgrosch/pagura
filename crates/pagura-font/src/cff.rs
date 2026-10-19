//! CFF and CFF2 containers: header, INDEXes, DICTs, charsets, FDArray and
//! FDSelect (TN 5176; OpenType CFF2).
use crate::budget::Work;
use crate::memory::{encoding, names, nested, tree, vector};
use crate::read::{self, glyph_id, limit, syntax, unsupported};
use crate::{GlyphId, features};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};
use pagura_raster::{Path, Transform};
use std::collections::BTreeMap;
use std::ops::Range;

mod dict;
mod index;
mod predefined;
mod select;
pub(crate) mod strings;
mod type2;
mod variation;

use dict::{Dict, Domain, Kind, Operator};
use index::{Index, OFFSET_SIZES};

/// The CFF major version (TN 5176 Table 1; the OpenType CFF2 header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    Cff1,
    Cff2,
}

/// The smallest header sizes (TN 5176 Table 1; CFF2 adds a Top DICT length).
const CFF1_HEADER: usize = 4;
const CFF2_HEADER: usize = 5;
/// Predefined charsets, named by small Top DICT charset offsets
/// (TN 5176 section 13 and Appendix C).
const ISO_ADOBE_CHARSET: usize = 0;
const EXPERT_CHARSET: usize = 1;
const EXPERT_SUBSET_CHARSET: usize = 2;
/// The ISOAdobe charset names SIDs 0-228 in order.
const ISO_ADOBE_GLYPHS: usize = 229;
/// Explicit charset formats (TN 5176 Tables 17-21).
const CHARSET_SIDS: u8 = 0;
const CHARSET_SHORT_RANGES: u8 = 1;
const CHARSET_RANGES: u8 = 2;
/// Top DICT defaults (TN 5176 Table 9): Type 2 charstrings, a 1/1000 em
/// FontMatrix scale and 8720 CIDs.
const TYPE2_CHARSTRINGS: f64 = 2.0;
const DEFAULT_SCALE: f64 = 0.001;
const DEFAULT_CID_COUNT: f64 = 8720.0;
/// FDArray sizes that FDSelect can address: 8-bit indices in CFF, 16-bit in
/// CFF2 (TN 5176 section 19).
const MAX_FONT_DICTS: usize = 256;
const MAX_CFF2_FONT_DICTS: usize = 65536;

#[derive(Debug)]
struct Private {
    subrs: Index,
    default_width: f64,
    nominal_width: f64,
    matrix: Transform,
    vsindex: usize,
    random_seed: u32,
}

#[derive(Debug)]
pub(crate) struct Cff {
    glyphs: Index,
    glyph_count: u32,
    global: Index,
    private: Vec<Private>,
    /// The Private DICT of each glyph; empty when every glyph uses the first.
    select: Vec<u16>,
    names: BTreeMap<Name, GlyphId>,
    glyph_names: Vec<Option<Name>>,
    cids: Option<BTreeMap<u32, GlyphId>>,
    encoding: Option<Box<[Option<Name>; 256]>>,
    matrix: Transform,
    version: Version,
    variations: Vec<Vec<f64>>,
}

/// Glyphs by name, and by CID for a CID-keyed font.
type CharsetMaps = (BTreeMap<Name, GlyphId>, Option<BTreeMap<u32, GlyphId>>);

/// The parts of a CFF program that its Top DICT locates.
struct Context<'a> {
    data: &'a [u8],
    table: Cursor<'a>,
    version: Version,
    strings: Index,
    variations: Vec<Vec<f64>>,
}

impl Cff {
    pub(crate) fn parse(data: &[u8], range: Range<usize>, work: &mut Work<'_>) -> Result<Self> {
        let start = Position::Buffer(range.start);
        let table = read::slice(data, range, "invalid CFF table")?;
        let Header {
            mut top,
            strings,
            global,
            version,
        } = header(table.clone(), data, work)?;
        if version == Version::Cff1 {
            top.validate(
                Domain::Top {
                    strings: strings.len(),
                },
                work,
            )?;
        }
        if top.has(Operator::SYNTHETIC_BASE) {
            return Err(unsupported(
                features::FORMAT,
                start,
                "synthetic CFF font is not supported",
            ));
        }
        let mut charstrings = table.cursor_at(top.offset(Operator::CHAR_STRINGS, None)?)?;
        let glyphs = Index::parse(&mut charstrings, version, work)?;
        if glyphs.is_empty() {
            return Err(charstrings.error("CFF has no glyphs"));
        }
        if version == Version::Cff1
            && top.number(Operator::CHARSTRING_TYPE, TYPE2_CHARSTRINGS)? != TYPE2_CHARSTRINGS
        {
            return Err(unsupported(
                features::FORMAT,
                start,
                "CFF CharstringType is not Type 2",
            ));
        }
        let variations = if version == Version::Cff2 && top.has(Operator::VSTORE) {
            variation::parse(table.cursor_at(top.offset(Operator::VSTORE, None)?)?, work)?
        } else {
            Vec::new()
        };
        let matrix = top.matrix(Transform::scale(DEFAULT_SCALE, DEFAULT_SCALE))?;
        let cid = top.has(Operator::ROS);
        let charset = match version {
            Version::Cff1 => charset(
                &table,
                top.offset(Operator::CHARSET, Some(ISO_ADOBE_CHARSET))?,
                glyphs.len(),
                work,
            )?,
            Version::Cff2 => Vec::new(),
        };
        let context = Context {
            data,
            table,
            version,
            strings,
            variations,
        };
        let (names, cids) = context.charset_maps(&top, &charset, work, start)?;
        let glyph_names = crate::names::indexed(
            glyphs.len(),
            names.iter().map(|(name, &glyph)| Ok((glyph, name))),
            work,
            start,
        )?;
        let encoding = if cid || version == Version::Cff2 {
            None
        } else {
            Some(strings::encoding(
                &context.table,
                top.offset(Operator::ENCODING, Some(0))?,
                &charset,
                &context.strings,
                data,
                work,
            )?)
        };
        let (private, select) = context.dictionaries(&top, glyphs.len(), work)?;
        Ok(Self {
            glyph_count: u32::try_from(glyphs.len())
                .map_err(|_| limit(start, "glyph count overflow"))?,
            glyphs,
            global,
            private,
            select,
            names,
            glyph_names,
            cids,
            encoding,
            matrix,
            version,
            variations: context.variations,
        })
    }

    pub(crate) const fn glyph_count(&self) -> u32 {
        self.glyph_count
    }

    pub(crate) const fn matrix(&self) -> Transform {
        self.matrix
    }

    pub(crate) const fn is_cid(&self) -> bool {
        self.cids.is_some()
    }

    pub(crate) fn cid(&self, cid: u32) -> Option<GlyphId> {
        self.cids.as_ref()?.get(&cid).copied()
    }

    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        crate::names::get(&self.glyph_names, glyph)
    }

    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.names.get(name).copied()
    }

    pub(crate) fn encoding(&self) -> Option<&[Option<Name>; 256]> {
        self.encoding.as_deref()
    }

    pub(crate) fn outline(&self, data: &[u8], glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
        type2::outline(self, data, glyph, work)
    }

    /// The charstring width; CFF2 charstrings carry none.
    pub(crate) fn advance(
        &self,
        data: &[u8],
        glyph: GlyphId,
        work: &mut Work<'_>,
    ) -> Result<Option<f64>> {
        match self.version {
            Version::Cff1 => type2::advance(self, data, glyph, work).map(Some),
            Version::Cff2 => Ok(None),
        }
    }

    /// The Private DICT that selects `glyph`'s subroutines and widths.
    fn private(&self, glyph: GlyphId) -> Option<&Private> {
        let fd = if self.select.is_empty() {
            0
        } else {
            usize::from(*self.select.get(u32_to_usize(glyph.0))?)
        };
        self.private.get(fd)
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.private
            .iter()
            .fold(vector(&self.private), |sum, p| {
                sum.saturating_add(p.subrs.heap_bytes())
            })
            .saturating_add(self.glyphs.heap_bytes())
            .saturating_add(self.global.heap_bytes())
            .saturating_add(vector(&self.select))
            .saturating_add(names(&self.names))
            .saturating_add(crate::names::heap_bytes(&self.glyph_names))
            .saturating_add(self.cids.as_ref().map_or(0, tree))
            .saturating_add(self.encoding.as_deref().map_or(0, encoding))
            .saturating_add(nested(&self.variations))
    }
}

struct Header {
    top: Dict,
    strings: Index,
    global: Index,
    version: Version,
}

fn header(mut r: Cursor<'_>, data: &[u8], work: &mut Work<'_>) -> Result<Header> {
    let version = match r.u8()? {
        1 => Version::Cff1,
        2 => Version::Cff2,
        _ => return Err(r.error("invalid CFF version")),
    };
    r.u8()?; // minor version
    let header = usize::from(r.u8()?);
    match version {
        Version::Cff2 => {
            let size = usize::from(r.u16()?);
            if header < CFF2_HEADER {
                return Err(r.error("invalid CFF2 header"));
            }
            r.seek(header)?;
            let top = Dict::parse(r.take_cursor(size)?, Kind::Cff2, work)?;
            let global = Index::parse(&mut r, version, work)?;
            Ok(Header {
                top,
                strings: Index::default(),
                global,
                version,
            })
        }
        Version::Cff1 => {
            let offset_size = r.u8()?;
            if header < CFF1_HEADER || !OFFSET_SIZES.contains(&offset_size) {
                return Err(r.error("invalid CFF header"));
            }
            r.seek(header)?;
            if Index::parse(&mut r, version, work)?.len() != 1 {
                return Err(r.error("embedded CFF must contain one font"));
            }
            let tops = Index::parse(&mut r, version, work)?;
            let (1, Some(top)) = (tops.len(), tops.get(data, 0)) else {
                return Err(r.error("CFF Top DICT count mismatch"));
            };
            let top = Dict::parse(top, Kind::Cff, work)?;
            let strings = Index::parse(&mut r, version, work)?;
            let global = Index::parse(&mut r, version, work)?;
            Ok(Header {
                top,
                strings,
                global,
                version,
            })
        }
    }
}

/// The SID (or CID) of each glyph (TN 5176 section 13).
fn charset(
    table: &Cursor<'_>,
    offset: usize,
    glyphs: usize,
    work: &mut Work<'_>,
) -> Result<Vec<u16>> {
    match offset {
        ISO_ADOBE_CHARSET => {
            let count = u16::try_from(glyphs)
                .ok()
                .filter(|&n| usize::from(n) <= ISO_ADOBE_GLYPHS)
                .ok_or_else(|| syntax(table.position(), "ISOAdobe charset glyph count exceeded"))?;
            return Ok((0..count).collect());
        }
        EXPERT_CHARSET | EXPERT_SUBSET_CHARSET => {
            let predefined = if offset == EXPERT_CHARSET {
                predefined::EXPERT
            } else {
                predefined::EXPERT_SUBSET
            };
            return predefined
                .get(..glyphs)
                .map(<[u16]>::to_vec)
                .ok_or_else(|| {
                    syntax(
                        table.position(),
                        "predefined CFF charset glyph count exceeded",
                    )
                });
        }
        _ => {}
    }
    let mut r = table.cursor_at(offset)?;
    let format = r.u8()?;
    work.entries(glyphs, r.position())?;
    let mut result = Vec::with_capacity(glyphs);
    result.push(0); // .notdef is implicit.
    while result.len() < glyphs {
        let first = r.u16()?;
        let left = match format {
            CHARSET_SIDS => 0,
            CHARSET_SHORT_RANGES => u16::from(r.u8()?),
            CHARSET_RANGES => r.u16()?,
            _ => return Err(r.error("invalid CFF charset format")),
        };
        let last = first
            .checked_add(left)
            .filter(|_| usize::from(left) < glyphs - result.len())
            .ok_or_else(|| r.error("CFF charset range exceeds glyph count"))?;
        result.extend(first..=last);
    }
    Ok(result)
}

impl Context<'_> {
    /// Glyph names, or CIDs for a CID-keyed font, from the charset.
    fn charset_maps(
        &self,
        top: &Dict,
        charset: &[u16],
        work: &mut Work<'_>,
        at: Position,
    ) -> Result<CharsetMaps> {
        let mut names = BTreeMap::new();
        if !top.has(Operator::ROS) {
            for (i, &sid) in charset.iter().enumerate() {
                let name = strings::sid(sid, &self.strings, self.data, work, at)?;
                if names.insert(name, glyph_id(i, at)?).is_some() {
                    return Err(syntax(at, "duplicate CFF glyph name"));
                }
            }
            return Ok((names, None));
        }
        let count = top.number(Operator::CID_COUNT, DEFAULT_CID_COUNT)?;
        let mut cids = BTreeMap::new();
        for (i, &cid) in charset.iter().enumerate() {
            if f64::from(cid) >= count {
                return Err(syntax(at, "CID outside CFF CIDCount"));
            }
            if cids.insert(u32::from(cid), glyph_id(i, at)?).is_some() {
                return Err(syntax(at, "duplicate CFF CID"));
            }
        }
        Ok((names, Some(cids)))
    }

    /// The Private DICTs and the FD of each glyph (TN 5176 section 19).
    fn dictionaries(
        &self,
        top: &Dict,
        glyphs: usize,
        work: &mut Work<'_>,
    ) -> Result<(Vec<Private>, Vec<u16>)> {
        let cff2 = self.version == Version::Cff2;
        let font_dicts = top.has(Operator::ROS) || cff2 && top.has(Operator::FD_ARRAY);
        if !font_dicts {
            let private = self.private_dict(top, Transform::IDENTITY, work)?;
            return Ok((vec![private], Vec::new()));
        }
        let mut array = self
            .table
            .cursor_at(top.offset(Operator::FD_ARRAY, None)?)?;
        let fonts = Index::parse(&mut array, self.version, work)?;
        let max = if cff2 {
            MAX_CFF2_FONT_DICTS
        } else {
            MAX_FONT_DICTS
        };
        if fonts.is_empty() || fonts.len() > max {
            return Err(array.error("invalid CFF FDArray count"));
        }
        let mut private = Vec::with_capacity(fonts.len());
        for font in fonts.ranges() {
            let at = Position::Buffer(font.start);
            work.work(font.len(), at)?;
            let bytes = read::slice(self.data, font, "CFF Font DICT outside font")?;
            let mut dict = Dict::parse(bytes, if cff2 { Kind::Cff2 } else { Kind::Cff }, work)?;
            if cff2 {
                if dict.has(Operator::FONT_MATRIX) {
                    // Secondary outline readers disagree for this unreviewed
                    // CFF2 combination. Keep it explicit until primary review.
                    return Err(unsupported(
                        features::FORMAT,
                        at,
                        "CFF2 Font DICT matrices are not supported",
                    ));
                }
            } else {
                dict.validate(
                    Domain::Top {
                        strings: self.strings.len(),
                    },
                    work,
                )?;
            }
            private.push(self.private_dict(&dict, dict.matrix(Transform::IDENTITY)?, work)?);
        }
        let select = if cff2 && !top.has(Operator::FD_SELECT) && private.len() == 1 {
            Vec::new()
        } else {
            select::parse(
                self.table
                    .cursor_at(top.offset(Operator::FD_SELECT, None)?)?,
                glyphs,
                private.len(),
                self.version,
                work,
            )?
        };
        Ok((private, select))
    }

    /// The Private DICT that `dict` (a Top or Font DICT) locates.
    fn private_dict(&self, dict: &Dict, matrix: Transform, work: &mut Work<'_>) -> Result<Private> {
        let at = self.table.position();
        let (size, offset) = match (dict.values(Operator::PRIVATE), self.version) {
            (Some(&[size, offset]), _) => (
                crate::charstring::integer(size, at)?,
                crate::charstring::integer(offset, at)?,
            ),
            (Some(_), _) => return Err(syntax(at, "invalid CFF Private operands")),
            // A CFF2 Font DICT without a Private DICT uses all defaults.
            (None, Version::Cff2) => (0, 0),
            (None, Version::Cff1) => return Err(syntax(at, "missing CFF Private DICT")),
        };
        let bytes = self.table.cursor_at(offset)?.take_cursor(size)?;
        let private_at = bytes.position();
        let mut private = match self.version {
            Version::Cff1 => Dict::parse(bytes, Kind::Cff, work)?,
            Version::Cff2 => Dict::parse(bytes, Kind::Cff2Private(&self.variations), work)?,
        };
        if self.version == Version::Cff1 {
            private.validate(Domain::Private, work)?;
        }
        private.check_blues(work)?;
        let subrs = if private.has(Operator::SUBRS) {
            let subrs_offset = offset
                .checked_add(private.offset(Operator::SUBRS, None)?)
                .ok_or_else(|| limit(private_at, "private offset overflow"))?;
            Index::parse(&mut self.table.cursor_at(subrs_offset)?, self.version, work)?
        } else {
            Index::default()
        };
        Ok(Private {
            subrs,
            default_width: private.number(Operator::DEFAULT_WIDTH_X, 0.0)?,
            nominal_width: private.number(Operator::NOMINAL_WIDTH_X, 0.0)?,
            matrix,
            vsindex: private.offset(Operator::VSINDEX, Some(0))?,
            random_seed: random_seed(private.number(Operator::INITIAL_RANDOM_SEED, 0.0)?),
        })
    }
}

// TN 5176 defines an initial seed, not a generator algorithm. Fold the numeric
// representation so arbitrary finite DICT numbers have reproducible seeds.
fn random_seed(value: f64) -> u32 {
    let [a, b, c, d, e, f, g, h] = value.to_bits().to_be_bytes();
    u32::from_be_bytes([a ^ e, b ^ f, c ^ g, d ^ h])
}
