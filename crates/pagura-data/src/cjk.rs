//! Opt-in, collection-specific Unicode substitutes; no assumption about CIDs.
use pagura_core::{Diagnostics, Limits, Recovery, Result};
use pagura_font::{FontFormat, FontProgram};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Debug)]
pub(crate) struct Fonts {
    limits: Limits,
    cache: Mutex<BTreeMap<&'static [u8], Arc<FontProgram>>>,
}

impl Fonts {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn get(&self, registry: &[u8], ordering: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        if registry != b"Adobe" {
            return Ok(None);
        }
        let Some((key, data)) = entry(ordering) else {
            return Ok(None);
        };
        if let Some(program) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Ok(Some(Arc::clone(program)));
        }
        // Parse outside the publication lock, just as for standard fonts.
        let program = Arc::new(FontProgram::parse_with_diagnostics(
            data,
            FontFormat::OpenType,
            &self.limits,
            &mut Diagnostics::new(Recovery::Strict),
        )?);
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Some(Arc::clone(cache.entry(key).or_insert(program))))
    }
}

fn entry(ordering: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    Some(match ordering {
        b"Japan1" => (
            b"Japan1",
            include_bytes!("../../../data/cjk/SourceHanSansJP-Regular.otf"),
        ),
        b"GB1" => (
            b"GB1",
            include_bytes!("../../../data/cjk/SourceHanSansCN-Regular.otf"),
        ),
        b"CNS1" => (
            b"CNS1",
            include_bytes!("../../../data/cjk/SourceHanSansTW-Regular.otf"),
        ),
        b"Korea1" => (
            b"Korea1",
            include_bytes!("../../../data/cjk/SourceHanSansKR-Regular.otf"),
        ),
        _ => return None,
    })
}
