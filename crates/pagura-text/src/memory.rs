//! Saturating retained-storage accounting (ADR 0012), once per loaded font.
use crate::{
    PdfFont,
    encoding::SimpleMapping,
    font::{Mapping, Ownership, Source},
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
            // The names' Unicode, found on first use, is bounded by its
            // names.
            .saturating_add(mapping.names.iter().flatten().fold(0_usize, |sum, name| {
                sum.saturating_add(
                    name.len()
                        .saturating_mul(crate::encoding::UNICODE_BYTES_PER_NAME_BYTE),
                )
            })),
        Mapping::Composite { map, shared: false } => size_of_val(map.as_ref())
            .saturating_add(2 * size_of::<usize>())
            .saturating_add(map.heap_bytes()),
        Mapping::Composite { shared: true, .. } => 0,
    };
    size_of::<PdfFont>()
        .saturating_add(mapping)
        .saturating_add(font.metrics.heap_bytes())
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
            ownership,
            cid_map,
            ..
        } => {
            let program_bytes = if *ownership == Ownership::Provider {
                0
            } else {
                program
                    .retained_bytes()
                    .saturating_add(size_of_val(program.as_ref()))
                    .saturating_add(2 * size_of::<usize>())
            };
            program_bytes.saturating_add(
                cid_map
                    .as_ref()
                    .map_or(0, crate::cid::CidToGidMap::heap_bytes),
            )
        }
    }
}

#[cfg(test)]
mod tests;
