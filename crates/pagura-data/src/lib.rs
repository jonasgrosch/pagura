#![forbid(unsafe_code)]
//! Optional, licensed bundled data for PDF font resources (ADR 0009).
//!
//! `standard-fonts` includes Adobe Core14 AFMs and Arimo/Tinos/Cousine and Foxit outlines;
//! `cmaps` includes Table 116 CMaps and four CID-to-Unicode collections. Both
//! features are enabled by default. With `default-features = false`, no data
//! bytes are linked; a host can implement [`Provider`] with lazy resources.
//!
//! `cjk-fonts` adds horizontal Source Han Sans substitutes and collection data;
//! it is opt-in and implies `cmaps`.
//!
//! Each instance owns bounded, lazy caches. No build script, runtime filesystem
//! access, network access or global mutable state is involved. Data provenance,
//! hashes and licences are recorded in `data/README.md` and `data/lock.json`.

use pagura_core::{Limits, Result};
use pagura_text::{CidToUnicode, PredefinedCMap, Provider, StandardFont};
use std::sync::Arc;

#[cfg(feature = "standard-fonts")]
const FONT_NOTICES: &[(&str, &str)] = &[
    (
        "Foxit Symbol and Dingbats",
        include_str!("../../../data/fonts/LICENSE-FOXIT"),
    ),
    (
        "Adobe Core 14 AFM files",
        include_str!("../../../data/afm/MustRead.html"),
    ),
    (
        "Arimo, Tinos and Cousine",
        include_str!("../../../data/fonts/NOTICE"),
    ),
    (
        "Apache License, Version 2.0",
        include_str!("../../../data/fonts/LICENSE-APACHE"),
    ),
];
#[cfg(not(feature = "standard-fonts"))]
const FONT_NOTICES: &[(&str, &str)] = &[];

#[cfg(feature = "cmaps")]
const CMAP_NOTICES: &[(&str, &str)] = &[(
    "Adobe cmap-resources",
    include_str!("../../../data/cmaps/LICENSE.md"),
)];
#[cfg(not(feature = "cmaps"))]
const CMAP_NOTICES: &[(&str, &str)] = &[];

#[cfg(feature = "cjk-fonts")]
const CJK_NOTICES: &[(&str, &str)] = &[(
    "Source Han Sans",
    include_str!("../../../data/cjk/LICENSE-OFL.txt"),
)];
#[cfg(not(feature = "cjk-fonts"))]
const CJK_NOTICES: &[(&str, &str)] = &[];

/// Licence notices of the data linked by the enabled features, as
/// `(subject, text)`. A binary enabling a data feature must reproduce them
/// with its documentation: BSD-3-Clause for the CMaps and Foxit programs,
/// SIL OFL 1.1 for Source Han Sans, Apache-2.0 and its
/// NOTICE for the substitute fonts, and Adobe's notice, which must accompany
/// the AFM data.
pub fn notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    FONT_NOTICES
        .iter()
        .chain(CMAP_NOTICES)
        .chain(CJK_NOTICES)
        .copied()
}

#[cfg(feature = "standard-fonts")]
mod afm;
#[cfg(feature = "cjk-fonts")]
mod cjk;
#[cfg(feature = "cmaps")]
mod cmaps;
#[cfg(feature = "standard-fonts")]
mod fonts;
#[cfg(feature = "cmaps")]
mod unicode;

/// Bundled font data with instance-local lazy caches. Resource parsing uses
/// the limits selected at construction, independent of any PDF's contents.
#[derive(Debug)]
pub struct Bundled {
    #[cfg(feature = "cjk-fonts")]
    cjk: cjk::Fonts,
    #[cfg(feature = "standard-fonts")]
    fonts: fonts::Fonts,
    #[cfg(feature = "cmaps")]
    cmaps: cmaps::CMaps,
}

impl Bundled {
    /// Creates empty caches with explicit parsing budgets. The inventory bounds
    /// cache keys (14 standard fonts, four CID substitutes, 59 CMaps,
    /// four Unicode tables); unknown names are
    /// never retained. Sharing this provider amortizes parsing across documents.
    pub fn new(limits: Limits) -> Self {
        let _ = limits;
        Self {
            #[cfg(feature = "cjk-fonts")]
            cjk: cjk::Fonts::new(limits),
            #[cfg(feature = "standard-fonts")]
            fonts: fonts::Fonts::new(limits),
            #[cfg(feature = "cmaps")]
            cmaps: cmaps::CMaps::new(limits),
        }
    }
}

impl Default for Bundled {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Provider for Bundled {
    fn cid_font(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> Result<Option<Arc<pagura_font::FontProgram>>> {
        let _ = (registry, ordering);
        #[cfg(feature = "cjk-fonts")]
        return self.cjk.get(registry, ordering);
        #[cfg(not(feature = "cjk-fonts"))]
        Ok(None)
    }
    fn standard_font(&self, base_name: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let _ = base_name;
        #[cfg(feature = "standard-fonts")]
        return self.fonts.get(base_name);
        #[cfg(not(feature = "standard-fonts"))]
        Ok(None)
    }
    fn predefined_cmap(&self, name: &[u8]) -> Result<Option<PredefinedCMap>> {
        let _ = name;
        #[cfg(feature = "cmaps")]
        return self.cmaps.get(name);
        #[cfg(not(feature = "cmaps"))]
        Ok(None)
    }
    fn cid_to_unicode(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> Result<Option<Arc<CidToUnicode>>> {
        let _ = (registry, ordering);
        #[cfg(feature = "cmaps")]
        return self.cmaps.unicode(registry, ordering);
        #[cfg(not(feature = "cmaps"))]
        Ok(None)
    }
}

#[cfg(all(test, any(feature = "cmaps", feature = "standard-fonts")))]
mod tests;

#[cfg(all(test, feature = "cjk-fonts"))]
mod cjk_tests;
#[cfg(all(test, feature = "standard-fonts"))]
mod symbol_tests;
