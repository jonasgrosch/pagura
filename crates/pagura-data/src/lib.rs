#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Optional, licensed bundled data for PDF font resources (ADR 0009).
//!
//! Each feature links one set of pinned files:
//!
//! | Feature | Data | Size |
//! |---|---|---|
//! | `standard-metrics` | Adobe Core14 AFM metrics: widths and descriptors of the standard 14 fonts | 0.6 MiB |
//! | `standard-outlines` | Arimo, Tinos, Cousine, FoxitSymbol and FoxitDingbats outlines for them; implies `standard-metrics` | 5.8 MiB |
//! | `cmaps` | the Table 116 CMaps and four collections' CID-to-Unicode data | 3.4 MiB |
//! | `cjk-fonts` | horizontal Source Han Sans substitutes for those collections; implies `cmaps` | 22.3 MiB |
//!
//! The first three are enabled by default. With `default-features = false`,
//! no data bytes are linked and every lookup is `None`; a host can implement
//! [`Provider`] with lazy resources instead. Without `standard-outlines`,
//! non-embedded standard fonts keep their metrics but have no outlines.
//!
//! Each instance owns bounded, lazy caches. No build script, runtime filesystem
//! access, network access or global mutable state is involved. Data provenance,
//! hashes and licences are recorded in `data/README.md` and `data/lock.json`.

use cache::Cache;
use pagura_core::{Limits, Result};
use pagura_font::FontProgram;
use pagura_text::{CidToUnicode, PredefinedCMap, Provider, StandardFont};
use std::sync::Arc;

mod afm;
mod cache;
mod cjk;
mod cmaps;
mod entry;
mod fonts;
#[cfg(all(test, any(feature = "standard-metrics", feature = "cmaps")))]
mod oracle;
mod unicode;

/// A licence notice and whether the enabled features link the data it covers.
struct Notice {
    subject: &'static str,
    text: &'static str,
    linked: bool,
}

const NOTICES: &[Notice] = &[
    Notice {
        subject: "Foxit Symbol and Dingbats",
        text: include_str!("../../../data/fonts/LICENSE-FOXIT"),
        linked: cfg!(feature = "standard-outlines"),
    },
    Notice {
        subject: "Adobe Core 14 AFM files",
        text: include_str!("../../../data/afm/MustRead.html"),
        linked: cfg!(feature = "standard-metrics"),
    },
    Notice {
        subject: "Arimo, Tinos and Cousine",
        text: include_str!("../../../data/fonts/NOTICE"),
        linked: cfg!(feature = "standard-outlines"),
    },
    Notice {
        subject: "Apache License, Version 2.0",
        text: include_str!("../../../data/fonts/LICENSE-APACHE"),
        linked: cfg!(feature = "standard-outlines"),
    },
    Notice {
        subject: "Adobe cmap-resources",
        text: include_str!("../../../data/cmaps/LICENSE.md"),
        linked: cfg!(feature = "cmaps"),
    },
    Notice {
        subject: "Source Han Sans",
        text: include_str!("../../../data/cjk/LICENSE-OFL.txt"),
        linked: cfg!(feature = "cjk-fonts"),
    },
];

/// Licence notices of the data linked by the enabled features, as
/// `(subject, text)`. A binary enabling a data feature must reproduce them
/// with its documentation: Adobe's notice, which must accompany the AFM
/// data; BSD-3-Clause for the Foxit programs and the CMaps; Apache-2.0 and
/// its NOTICE for the substitute fonts; SIL OFL 1.1 for Source Han Sans.
pub fn notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    NOTICES
        .iter()
        .filter(|notice| notice.linked)
        .map(|notice| (notice.subject, notice.text))
}

/// Bundled font data with instance-local lazy caches. Resource parsing uses
/// the limits selected at construction, independent of any PDF's contents.
#[derive(Debug)]
pub struct Bundled {
    limits: Limits,
    standard_fonts: Cache<StandardFont>,
    cid_fonts: Cache<FontProgram>,
    cmaps: Cache<PredefinedCMap>,
    cid_to_unicode: Cache<CidToUnicode>,
}

impl Bundled {
    /// Creates empty caches with explicit parsing budgets. The inventory bounds
    /// cache keys (14 standard fonts, four CID substitutes, 59 CMaps,
    /// four Unicode tables); unknown names are
    /// never retained. Sharing this provider amortizes parsing across documents.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            standard_fonts: Cache::new(),
            cid_fonts: Cache::new(),
            cmaps: Cache::new(),
            cid_to_unicode: Cache::new(),
        }
    }
}

impl Default for Bundled {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Provider for Bundled {
    fn cid_font(&self, registry: &[u8], ordering: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        cjk::cid_font(registry, ordering, &self.cid_fonts, &self.limits)
    }
    fn standard_font(&self, base_name: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        fonts::standard_font(base_name, &self.standard_fonts, &self.limits)
    }
    fn predefined_cmap(&self, name: &[u8]) -> Result<Option<PredefinedCMap>> {
        cmaps::predefined_cmap(name, &self.cmaps, &self.limits)
    }
    fn cid_to_unicode(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> Result<Option<Arc<CidToUnicode>>> {
        cmaps::cid_to_unicode(registry, ordering, &self.cid_to_unicode, &self.limits)
    }
}

/// The ordering of an Adobe character collection (9.7.3). Every bundled
/// collection is registered by Adobe, so other registries have no data.
fn adobe_ordering<'a>(registry: &[u8], ordering: &'a [u8]) -> Option<&'a [u8]> {
    (registry == b"Adobe").then_some(ordering)
}

/// The lines of a text file with the byte offset each starts at, without
/// their `\n` or `\r\n` terminators.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split_inclusive('\n').scan(0, |next, line| {
        let offset = *next;
        *next += line.len();
        let line = line.strip_suffix('\n').unwrap_or(line);
        Some((offset, line.strip_suffix('\r').unwrap_or(line)))
    })
}

#[cfg(test)]
mod tests {
    use super::{Bundled, notices};
    use pagura_core::{ErrorKind, Limits, Result};
    use pagura_text::Provider;

    #[test]
    fn unavailable_resources_are_explicit_and_not_cached_as_guesses() -> Result<()> {
        let provider = Bundled::default();
        assert!(provider.cid_font(b"Unknown", b"Japan1")?.is_none());
        assert!(provider.cid_font(b"Adobe", b"Unknown")?.is_none());
        assert!(provider.standard_font(b"Unknown")?.is_none());
        assert!(provider.predefined_cmap(b"Unknown")?.is_none());
        assert!(provider.cid_to_unicode(b"Unknown", b"Japan1")?.is_none());
        assert!(provider.cid_to_unicode(b"Adobe", b"Unknown")?.is_none());
        assert_eq!(
            provider.cid_font(b"Adobe", b"Japan1")?.is_some(),
            cfg!(feature = "cjk-fonts")
        );
        assert_eq!(
            provider.standard_font(b"Helvetica")?.is_some(),
            cfg!(feature = "standard-metrics")
        );
        assert_eq!(
            provider.predefined_cmap(b"90ms-RKSJ-H")?.is_some(),
            cfg!(feature = "cmaps")
        );
        // Only resources the inventory names are cached, and the provider's
        // debug output lists their names rather than their bytes.
        let debug = format!("{provider:?}");
        assert!(debug.len() < 4096, "{debug}");
        if cfg!(feature = "standard-metrics") {
            assert!(
                debug.contains(r#"standard_fonts: {"Helvetica"}"#),
                "{debug}"
            );
        }
        assert!(!debug.contains("Unknown"), "{debug}");
        Ok(())
    }

    #[test]
    fn parsing_budgets_fail_before_publication() {
        // Each font budget is zeroed together with the general field that
        // the pagura-text and pagura-font parsers of these files have used.
        for limits in [
            Limits {
                font_work: 0,
                operations: 0,
                ..Limits::default()
            },
            Limits {
                font_entries: 0,
                value_nodes: 0,
                ..Limits::default()
            },
            Limits {
                decoded_bytes: 0,
                ..Limits::default()
            },
        ] {
            let provider = Bundled::new(limits);
            let results = [
                provider.cid_font(b"Adobe", b"Japan1").map(|_| ()),
                provider.standard_font(b"Helvetica").map(|_| ()),
                provider.predefined_cmap(b"90ms-RKSJ-H").map(|_| ()),
                provider.cid_to_unicode(b"Adobe", b"Japan1").map(|_| ()),
            ];
            let linked = [
                cfg!(feature = "cjk-fonts"),
                cfg!(feature = "standard-metrics"),
                cfg!(feature = "cmaps"),
                cfg!(feature = "cmaps"),
            ];
            for (result, linked) in results.into_iter().zip(linked) {
                match result {
                    Err(error) => assert_eq!(error.kind, ErrorKind::Limit),
                    Ok(()) => assert!(!linked, "{limits:?}"),
                }
            }
            // Nothing was published: the debug output names no cached key.
            assert_eq!(format!("{provider:?}").matches('"').count(), 0);
        }
    }

    #[test]
    fn notices_accompany_every_enabled_data_set() {
        let notices: Vec<_> = notices().collect();
        let has = |subject: &str, phrase: &str| {
            notices
                .iter()
                .any(|(name, text)| *name == subject && text.contains(phrase))
        };
        for (linked, subject, phrase) in [
            (
                cfg!(feature = "standard-metrics"),
                "Adobe Core 14 AFM files",
                "not distributed without this file",
            ),
            (
                cfg!(feature = "standard-outlines"),
                "Arimo, Tinos and Cousine",
                "Apache License, Version 2.0",
            ),
            (
                cfg!(feature = "standard-outlines"),
                "Apache License, Version 2.0",
                "TERMS AND CONDITIONS",
            ),
            (
                cfg!(feature = "standard-outlines"),
                "Foxit Symbol and Dingbats",
                "Redistributions in binary form",
            ),
            (
                cfg!(feature = "cmaps"),
                "Adobe cmap-resources",
                "Redistributions in binary form",
            ),
            (
                cfg!(feature = "cjk-fonts"),
                "Source Han Sans",
                "SIL OPEN FONT LICENSE",
            ),
        ] {
            assert_eq!(has(subject, phrase), linked, "{subject}");
            assert_eq!(
                notices.iter().any(|(name, _)| *name == subject),
                linked,
                "{subject}"
            );
        }
    }
}
