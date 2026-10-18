//! Saturating retained-storage accounting (ADR 0012), once per loaded font.
use crate::{
    PdfFont,
    encoding::SimpleMapping,
    font::{Mapping, Source},
};

pub(crate) fn tree<K, V>(entries: usize) -> usize {
    entries.saturating_mul(
        size_of::<K>()
            .saturating_add(size_of::<V>())
            .saturating_add(64),
    )
}

pub(crate) fn estimate(font: &PdfFont) -> usize {
    let mapping = match &font.mapping {
        Mapping::Simple(mapping) => size_of::<SimpleMapping>()
            .saturating_add(
                mapping
                    .names
                    .iter()
                    .flatten()
                    .fold(0_usize, |sum, name| sum.saturating_add(name.len())),
            )
            .saturating_add(
                mapping
                    .unicode
                    .iter()
                    .flatten()
                    .fold(0_usize, |sum, text| sum.saturating_add(text.capacity())),
            ),
        Mapping::Composite { map, shared: false } => size_of_val(map.as_ref())
            .saturating_add(2 * size_of::<usize>())
            .saturating_add(map.heap_bytes()),
        Mapping::Composite { shared: true, .. } => 0,
    };
    size_of::<PdfFont>()
        .saturating_add(mapping)
        .saturating_add(font.metrics.heap_bytes())
        .saturating_add(
            font.to_unicode
                .as_ref()
                .map_or(0, crate::cmap::CMap::heap_bytes),
        )
        .saturating_add(source(&font.source))
}

fn source(source: &Source) -> usize {
    match source {
        Source::MetricsOnly | Source::CidSubstitute { .. } => 0,
        Source::Type3 {
            procedures,
            resources,
            ..
        } => tree::<pagura_core::Name, pagura_core::ObjectId>(procedures.len())
            .saturating_add(
                procedures
                    .keys()
                    .fold(0_usize, |sum, name| sum.saturating_add(name.len())),
            )
            .saturating_add(resources.as_ref().map_or(0, |dict| {
                dict.retained_bytes().saturating_sub(size_of_val(dict))
            })),
        Source::Program {
            program,
            substituted,
            cid_map,
            ..
        } => {
            let program_bytes = if *substituted {
                0
            } else {
                // FontProgram's internals are private. This explicit proxy
                // allows for decoded programs, offsets, charstrings, name/CID
                // indexes and encodings. No outlines are retained by that API.
                program
                    .data()
                    .len()
                    .saturating_mul(2)
                    .saturating_add(
                        usize::try_from(program.glyph_count())
                            .unwrap_or(usize::MAX)
                            .saturating_mul(192),
                    )
                    .saturating_add(16 * 1024)
                    .saturating_add(size_of_val(program.as_ref()))
                    .saturating_add(6 * size_of::<usize>())
            };
            program_bytes.saturating_add(cid_map.as_ref().map_or(0, Vec::capacity))
        }
    }
}

#[cfg(test)]
mod tests;
