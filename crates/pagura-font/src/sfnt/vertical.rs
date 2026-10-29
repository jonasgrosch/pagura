//! Selected OpenType GSUB vertical single substitutions.
//!
//! OpenType 1.9.1, GSUB types 1/7 and Layout Common Table Formats. Only the
//! chosen script/language's vrt2 (preferred) or vert lookups are evaluated.
//! PDF glyph selection and advances remain in pagura-text.
use crate::{GlyphId, VerticalSubstitution, budget::Work, features};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};
use std::collections::{BTreeMap, btree_map::Entry};

type Mapping = BTreeMap<u16, u16>;

pub(super) fn parse(
    mut table: Cursor<'_>,
    script: [u8; 4],
    language: Option<[u8; 4]>,
    glyphs: u32,
    work: &mut Work<'_>,
) -> Result<Option<VerticalSubstitution>> {
    let major = table.u16()?;
    let minor = table.u16()?;
    if major != 1 || minor > 1 {
        return Err(table.error("invalid GSUB version"));
    }
    let scripts = next_offset(&mut table)?;
    let features = next_offset(&mut table)?;
    let lookups = next_offset(&mut table)?;
    let variations = if minor == 1 { table.u32()? } else { 0 };
    let Some(script) = tagged(scripts, script, Some(*b"DFLT"), work)? else {
        return Ok(None);
    };
    let mut header = script.clone();
    let default = header.u16()?;
    let selected = match language {
        Some(language) => tagged(header, language, None, work)?,
        None => None,
    };
    let mut lang = if let Some(selected) = selected {
        selected
    } else if default != 0 {
        required(&script, default)?
    } else {
        return Ok(None);
    };
    if lang.u16()? != 0 {
        return Err(lang.error("nonzero reserved LangSys lookup order"));
    }
    let required_feature = lang.u16()?;
    let count = usize::from(lang.u16()?);
    work.entries(
        count + usize::from(required_feature != u16::MAX),
        lang.position(),
    )?;
    work.work(count, lang.position())?;
    let mut indices = Vec::with_capacity(count + usize::from(required_feature != u16::MAX));
    if required_feature != u16::MAX {
        indices.push(required_feature);
    }
    for _ in 0..count {
        indices.push(lang.u16()?);
    }
    let selected = select_features(features, &indices, work)?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    if variations != 0 {
        return Err(Error::unsupported(
            features::VARIATIONS,
            "vertical substitution feature variations are unavailable",
        )
        .at(table.position()));
    }
    let mut result = Mapping::new();
    for index in selected {
        let lookup = indexed(lookups.clone(), index)?;
        let next = lookup_mapping(lookup, glyphs, work)?;
        // Lookups act in LookupList order on the preceding lookup's result.
        // Subtables within one lookup instead use their first matching input.
        for output in result.values_mut() {
            work.work(1, table.position())?;
            if let Some(replacement) = next.get(output) {
                *output = *replacement;
            }
        }
        for (input, output) in next {
            work.work(1, table.position())?;
            if let Entry::Vacant(entry) = result.entry(input) {
                work.entries(1, table.position())?;
                entry.insert(output);
            }
        }
    }
    let pairs = result
        .into_iter()
        .filter(|(input, output)| input != output)
        .collect();
    Ok(Some(VerticalSubstitution { pairs }))
}

/// Tag records are sorted; fallback is used only if the requested tag is absent.
fn tagged<'a>(
    mut list: Cursor<'a>,
    tag: [u8; 4],
    fallback: Option<[u8; 4]>,
    work: &mut Work<'_>,
) -> Result<Option<Cursor<'a>>> {
    let base = list.clone();
    let count = usize::from(list.u16()?);
    work.work(count, list.position())?;
    let mut previous = None;
    let mut found = None;
    let mut default = None;
    for _ in 0..count {
        let current = list.take_array::<4>()?;
        let offset = list.u16()?;
        if previous.is_some_and(|previous| previous >= current) {
            return Err(list.error("unordered or duplicate layout tags"));
        }
        previous = Some(current);
        if current == tag {
            found = Some(offset);
        } else if Some(current) == fallback {
            default = Some(offset);
        }
    }
    found
        .or(default)
        .map(|offset| required(&base, offset))
        .transpose()
}

fn select_features(
    mut features: Cursor<'_>,
    indices: &[u16],
    work: &mut Work<'_>,
) -> Result<Option<Vec<u16>>> {
    let base = features.clone();
    let count = usize::from(features.u16()?);
    let record = |index: u16| -> Result<([u8; 4], u16)> {
        if usize::from(index) >= count {
            return Err(features.error("LangSys feature index outside FeatureList"));
        }
        let mut record = base.cursor_at(2 + usize::from(index) * 6)?;
        Ok((record.take_array::<4>()?, record.u16()?))
    };
    let mut selected = None;
    for &index in indices {
        work.work(1, features.position())?;
        let (tag, _) = record(index)?;
        if &tag == b"vrt2" || (&tag == b"vert" && selected.is_none()) {
            selected = Some(tag);
        }
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    let mut result = Vec::new();
    for &index in indices {
        work.work(1, features.position())?;
        let (tag, offset) = record(index)?;
        if tag != selected {
            continue;
        }
        let mut feature = required(&base, offset)?;
        if feature.u16()? != 0 {
            return Err(feature.error("nonzero vertical feature parameters"));
        }
        let count = usize::from(feature.u16()?);
        work.entries(count, feature.position())?;
        work.work(count, feature.position())?;
        for _ in 0..count {
            result.push(feature.u16()?);
        }
    }
    result.sort_unstable();
    result.dedup();
    Ok(Some(result))
}

fn indexed(mut list: Cursor<'_>, index: u16) -> Result<Cursor<'_>> {
    let count = list.u16()?;
    if index >= count {
        return Err(list.error("feature lookup index outside LookupList"));
    }
    list.skip(usize::from(index) * 2)?;
    next_offset(&mut list)
}

fn next_offset<'a>(base: &mut Cursor<'a>) -> Result<Cursor<'a>> {
    let offset = base.u16()?;
    required(base, offset)
}

fn required<'a>(base: &Cursor<'a>, offset: u16) -> Result<Cursor<'a>> {
    if offset == 0 {
        return Err(base.error("null required layout offset"));
    }
    base.cursor_at(usize::from(offset))
}

fn lookup_mapping(mut lookup: Cursor<'_>, glyphs: u32, work: &mut Work<'_>) -> Result<Mapping> {
    let kind = lookup.u16()?;
    let flags = lookup.u16()?;
    let count = usize::from(lookup.u16()?);
    if kind == 0 || kind > 8 {
        return Err(lookup.error("invalid GSUB lookup type"));
    }
    if flags & 0x00e0 != 0 {
        return Err(lookup.error("reserved GSUB lookup flags"));
    }
    // RightToLeft affects GPOS cursive attachment only. Filtering needs GDEF.
    if !matches!(kind, 1 | 7) || flags & !1 != 0 {
        return Err(
            Error::unsupported(features::FORMAT, "vertical GSUB lookup is unavailable")
                .at(lookup.position()),
        );
    }
    work.work(count, lookup.position())?;
    let mut mapping = Mapping::new();
    for _ in 0..count {
        let mut subtable = next_offset(&mut lookup)?;
        if kind == 7 {
            let mut extension = subtable.clone();
            if extension.u16()? != 1 {
                return Err(extension.error("invalid GSUB extension format"));
            }
            let kind = extension.u16()?;
            if kind == 0 || kind == 7 || kind > 8 {
                return Err(extension.error("invalid GSUB extension lookup type"));
            }
            if kind != 1 {
                return Err(Error::unsupported(
                    features::FORMAT,
                    "vertical GSUB extension is unavailable",
                )
                .at(extension.position()));
            }
            let offset = u32_to_usize(extension.u32()?);
            if offset == 0 {
                return Err(extension.error("null GSUB extension offset"));
            }
            subtable = subtable.cursor_at(offset)?;
        }
        single(subtable, glyphs, &mut mapping, work)?;
    }
    Ok(mapping)
}

fn single(
    mut subtable: Cursor<'_>,
    glyphs: u32,
    mapping: &mut Mapping,
    work: &mut Work<'_>,
) -> Result<()> {
    let format = subtable.u16()?;
    let mut coverage = next_offset(&mut subtable)?;
    let value = subtable.u16()?;
    if !matches!(format, 1 | 2) {
        return Err(subtable.error("invalid single substitution format"));
    }
    let mut index = 0_usize;
    let mut previous = None;
    let coverage_format = coverage.u16()?;
    if !matches!(coverage_format, 1 | 2) {
        return Err(coverage.error("invalid coverage format"));
    }
    let count = usize::from(coverage.u16()?);
    work.work(count, coverage.position())?;
    for _ in 0..count {
        let start = coverage.u16()?;
        let end = match coverage_format {
            1 => start,
            2 => {
                let end = coverage.u16()?;
                if usize::from(coverage.u16()?) != index {
                    return Err(coverage.error("nonsequential coverage index"));
                }
                end
            }
            _ => return Err(coverage.error("invalid coverage format")),
        };
        if start > end
            || previous.is_some_and(|previous| previous >= start)
            || u32::from(end) >= glyphs
        {
            return Err(coverage.error("invalid or unordered coverage glyphs"));
        }
        previous = Some(end);
        for input in start..=end {
            work.work(1, coverage.position())?;
            let output = if format == 1 {
                input.wrapping_add(value)
            } else {
                if index >= usize::from(value) {
                    return Err(subtable.error("single substitution count disagrees with coverage"));
                }
                subtable.u16()?
            };
            if u32::from(output) >= glyphs {
                return Err(subtable.error("substitute glyph outside font"));
            }
            if let Entry::Vacant(entry) = mapping.entry(input) {
                work.entries(1, coverage.position())?;
                entry.insert(output);
            }
            index += 1;
        }
    }
    if format == 2 && index != usize::from(value) {
        return Err(subtable.error("single substitution count disagrees with coverage"));
    }
    Ok(())
}

impl VerticalSubstitution {
    /// Applies the selected vertical lookups once; glyphs without alternates stay unchanged.
    pub fn apply(&self, glyph: GlyphId) -> GlyphId {
        u16::try_from(glyph.0)
            .ok()
            .and_then(|input| {
                self.pairs
                    .binary_search_by_key(&input, |&(input, _)| input)
                    .ok()
            })
            .and_then(|index| self.pairs.get(index))
            .map_or(glyph, |&(_, output)| GlyphId(u32::from(output)))
    }

    /// Heap bytes owned by the compiled map, excluding the value itself.
    pub fn retained_bytes(&self) -> usize {
        crate::memory::vector(&self.pairs)
    }
}

#[cfg(test)]
mod tests;
