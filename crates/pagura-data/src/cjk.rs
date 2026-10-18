//! Opt-in, collection-specific Unicode substitutes for CID fonts. Selection
//! goes through the collection's Unicode data, never the fonts' own CIDs.
use crate::{cache::Cache, entry::Program};
use pagura_core::{Limits, Result};
#[cfg(feature = "cjk-fonts")]
use pagura_font::FontFormat;
use pagura_font::FontProgram;
use std::sync::Arc;

/// The substitute for an Adobe character collection, if one is linked.
pub(crate) fn cid_font(
    registry: &[u8],
    ordering: &[u8],
    cache: &Cache<FontProgram>,
    limits: &Limits,
) -> Result<Option<Arc<FontProgram>>> {
    let Some(program) = crate::adobe_ordering(registry, ordering)
        .and_then(|ordering| PROGRAMS.iter().find(|program| program.key == ordering))
    else {
        return Ok(None);
    };
    cache
        .get_or_try_insert(program.key, || program.parse(limits))
        .map(Some)
}

/// Source Han Sans Regular, one regional subset per collection.
#[cfg(feature = "cjk-fonts")]
pub(crate) const PROGRAMS: &[Program] = &[
    Program {
        key: b"Japan1",
        data: include_bytes!("../../../data/cjk/SourceHanSansJP-Regular.otf"),
        format: FontFormat::OpenType,
    },
    Program {
        key: b"GB1",
        data: include_bytes!("../../../data/cjk/SourceHanSansCN-Regular.otf"),
        format: FontFormat::OpenType,
    },
    Program {
        key: b"CNS1",
        data: include_bytes!("../../../data/cjk/SourceHanSansTW-Regular.otf"),
        format: FontFormat::OpenType,
    },
    Program {
        key: b"Korea1",
        data: include_bytes!("../../../data/cjk/SourceHanSansKR-Regular.otf"),
        format: FontFormat::OpenType,
    },
];
#[cfg(not(feature = "cjk-fonts"))]
pub(crate) const PROGRAMS: &[Program] = &[];

#[cfg(all(test, feature = "cjk-fonts"))]
mod tests {
    use crate::Bundled;
    use pagura_core::{Error, Result};
    use pagura_text::Provider;
    use std::sync::Arc;

    #[test]
    fn every_collection_substitute_loads_once() -> Result<()> {
        let provider = Bundled::default();
        for program in super::PROGRAMS {
            let first = provider
                .cid_font(b"Adobe", program.key)?
                .ok_or_else(|| Error::syntax("missing CJK program"))?;
            let again = provider
                .cid_font(b"Adobe", program.key)?
                .ok_or_else(|| Error::syntax("missing cached CJK program"))?;
            assert!(Arc::ptr_eq(&first, &again));
            assert_eq!(first.data(), program.data);
        }
        Ok(())
    }
}
