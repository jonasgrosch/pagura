//! Glyph selection: from a code, name or CID to a drawable shape (9.6.5, 9.7.6).
use super::{FontKind, Mapping, Ownership, PdfFont, Source};
use crate::{
    GlyphShape,
    cid::{Cid, CidToGidMap},
    cmap::{CMap, Code},
};
use pagura_core::{Meter, Name, Result};
use pagura_font::{FontProgram, GlyphId};
use std::sync::Arc;

impl PdfFont {
    /// The shape of the glyph `code` selects; `unicode` gives the code's
    /// Unicode text, which TrueType selection reads the first time a code
    /// is selected.
    pub(crate) fn shape<'u>(
        &self,
        code: Code,
        selector: u32,
        name: Option<&Name>,
        unicode: impl FnOnce() -> Option<&'u str>,
    ) -> GlyphShape {
        match &self.source {
            Source::MetricsOnly => GlyphShape::Blank,
            Source::CidSubstitute { program, unicode } => Cid::try_from(selector)
                .ok()
                .and_then(|cid| crate::substitute::cid::glyph(program, Some(unicode), cid))
                .map_or(GlyphShape::Blank, |glyph| GlyphShape::Outline {
                    program: Arc::clone(program),
                    glyph,
                    matrix: program.font_matrix(),
                }),
            Source::Type3 {
                procedures, matrix, ..
            } => name
                .and_then(|name| procedures.get(name))
                .map_or(GlyphShape::Blank, |id| GlyphShape::Type3 {
                    procedure: *id,
                    matrix: *matrix,
                }),
            Source::Program {
                program,
                route,
                ownership,
                cid_map,
            } => {
                let select = || {
                    if (self.kind == FontKind::TrueType || *ownership == Ownership::Provider)
                        && program.format() != pagura_font::FontFormat::Cff
                    {
                        crate::selection::truetype(program, code.value, name, unicode(), *route)
                    } else {
                        name.and_then(|name| program.glyph_for_name(name.as_bytes()))
                    }
                };
                let glyph = if self.kind == FontKind::Type0 {
                    Cid::try_from(selector)
                        .ok()
                        .and_then(|cid| cid_glyph(program, cid_map.as_ref(), cid))
                } else if let Mapping::Simple(simple) = &self.mapping
                    && code.len == 1
                    && let Some(slot) = usize::try_from(code.value)
                        .ok()
                        .and_then(|c| simple.selected.get(c))
                {
                    *slot.get_or_init(select)
                } else {
                    select()
                };
                GlyphShape::Outline {
                    program: Arc::clone(program),
                    glyph: glyph.unwrap_or(GlyphId(0)),
                    matrix: program.font_matrix(),
                }
            }
        }
    }

    /// The CID a code selects (9.7.6.3), given the CID its CMap maps it to.
    pub(super) fn select_cid(
        &self,
        map: &CMap,
        code: Code,
        mapped: Option<Cid>,
        work: &mut Meter,
    ) -> Result<Cid> {
        let notdef = match mapped {
            // A CIDToGIDMap stream without an entry for the CID selects CID 0,
            // before ordinary notdef substitution is considered.
            Some(cid) if self.lacks_gid_entry(cid) => return Ok(Cid::NOTDEF),
            // A mapped CID the font can draw is used as it is.
            Some(cid) if self.has_cid(cid) => return Ok(cid),
            // An undefined code or a CID without a glyph uses the code's notdef
            // mapping.
            _ => map.notdef(code, work)?,
        };
        // A notdef CID without a glyph selects CID 0.
        Ok(if self.has_cid(notdef) {
            notdef
        } else {
            Cid::NOTDEF
        })
    }

    fn lacks_gid_entry(&self, cid: Cid) -> bool {
        match &self.source {
            Source::Program {
                cid_map: Some(map), ..
            } => map.get(cid).is_none(),
            _ => false,
        }
    }

    fn has_cid(&self, cid: Cid) -> bool {
        match &self.source {
            Source::Program {
                program, cid_map, ..
            } => cid_glyph(program, cid_map.as_ref(), cid).is_some(),
            Source::CidSubstitute { .. } => true,
            Source::Type3 { .. } | Source::MetricsOnly => false,
        }
    }
}

fn cid_glyph(program: &FontProgram, map: Option<&CidToGidMap>, cid: Cid) -> Option<GlyphId> {
    if program.is_cid_keyed() {
        return program.glyph_for_cid(u32::from(cid));
    }
    let glyph = mapped_gid(map, cid)?;
    (glyph < program.glyph_count()).then_some(GlyphId(glyph))
}

/// The glyph of `cid` under the CIDToGIDMap, or Identity without one
/// (9.7.4.2, Table 115). `None` means the stream has no entry for it.
fn mapped_gid(map: Option<&CidToGidMap>, cid: Cid) -> Option<u32> {
    match map {
        Some(map) => map.get(cid).map(u32::from),
        None => Some(u32::from(cid)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::load::cid_map;
    use super::*;
    use crate::objects::array;
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Error, ObjectId, Result};
    use pagura_document::{Document, OpenOptions};
    use pagura_testkit::fixture;

    #[test]
    fn cid_to_gid_vector_and_identity() -> Result<()> {
        let document = Document::open(fixture("text", "cid-to-gid.pdf"), &OpenOptions::default())?;
        let dict = document
            .object(ObjectId::new(4, 0))?
            .dictionary()
            .ok_or_else(|| Error::syntax("missing dictionary"))?;
        let map = cid_map(&document, dict, Some(ObjectId::new(4, 0)))?;
        let data = fixture("text", "cid-to-gid.out");
        let expected = Parser::new(&data, Origin::Buffer, document.limits()).parse()?;
        for value in array(&expected)? {
            let values = array(value)?;
            let cid = values[0]
                .as_integer()
                .and_then(Cid::from_integer)
                .ok_or_else(|| Error::syntax("invalid oracle code"))?;
            assert_eq!(
                i64::from(mapped_gid(map.as_ref(), cid).unwrap_or(0)),
                values[1]
                    .as_integer()
                    .ok_or_else(|| Error::syntax("invalid oracle glyph"))?
            );
            assert_eq!(mapped_gid(None, cid), Some(u32::from(cid)));
        }
        Ok(())
    }
}
