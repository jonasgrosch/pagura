//! Pinned Table 116 CMaps and the CID-to-Unicode data of their collections.
//!
//! The pinned files are trusted data that no PDF supplies, so each is parsed
//! once per process, at its first use, under the fixed default limits rather
//! than a provider's, and every provider shares the result. The files are
//! static data and are not copied; what a parsed table keeps is listed with
//! each loader.
use crate::{entry::File, unicode};
use pagura_core::{Error, Limits, Result};
use pagura_text::{CidToUnicode, PredefinedCMap};
use std::sync::{Arc, OnceLock};

/// The predefined CMap `name`, with its `usecmap` parents resolved.
///
/// A parsed CMap keeps its ranges, with its parents' kept once: from 0.6 KB
/// to 0.98 MB (UniCNS-UTF16-H), 6.6 MB for all 59 natively and 6.4 MB on
/// wasm32; parsing needs little beyond what it keeps.
pub(crate) fn predefined_cmap(name: &[u8]) -> Result<Option<PredefinedCMap>> {
    load(name, &mut Vec::new())
}

/// Loads `name`, where `path` holds the CMaps whose `usecmap` led here.
fn load(name: &[u8], path: &mut Vec<&'static [u8]>) -> Result<Option<PredefinedCMap>> {
    static PARSED: [OnceLock<Result<PredefinedCMap>>; CMAPS.len()] =
        [const { OnceLock::new() }; CMAPS.len()];
    let Some(index) = CMAPS.iter().position(|file| file.key == name) else {
        return Ok(None);
    };
    let (file, limits) = (&CMAPS[index], Limits::default());
    if path.contains(&file.key) {
        return Err(Error::cycle("predefined usecmap cycle"));
    }
    if path.len() >= limits.depth {
        return Err(Error::limit("predefined CMap depth exceeded"));
    }
    PARSED[index]
        .get_or_init(|| {
            path.push(file.key);
            let cmap = PredefinedCMap::parse(file.data, &limits, |parent| {
                load(parent, path)?
                    .ok_or_else(|| Error::syntax("missing predefined usecmap parent"))
            });
            path.pop();
            cmap
        })
        .clone()
        .map(Some)
}

/// CID-to-Unicode data for an Adobe character collection (9.10.2).
///
/// A collection's table keeps 0.70 MB (Korea1) to 1.21 MB (GB1) natively and
/// half that on wasm32, 3.4 MB and 1.7 MB for all four; parsing one peaks at
/// about 1.9 times what it keeps.
pub(crate) fn cid_to_unicode(
    registry: &[u8],
    ordering: &[u8],
) -> Result<Option<Arc<CidToUnicode>>> {
    static PARSED: [OnceLock<Result<Arc<CidToUnicode>>>; UNICODE.len()] =
        [const { OnceLock::new() }; UNICODE.len()];
    let Some(index) = crate::adobe_ordering(registry, ordering)
        .and_then(|ordering| UNICODE.iter().position(|file| file.key == ordering))
    else {
        return Ok(None);
    };
    PARSED[index]
        .get_or_init(|| unicode::parse(UNICODE[index].data, &Limits::default()).map(Arc::new))
        .clone()
        .map(Some)
}

/// Adobe's `cmap-resources` CMaps, by CMap name.
#[cfg(feature = "cmaps")]
pub(crate) const CMAPS: &[File] = &[
    File {
        key: b"B5pc-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/B5pc-H"),
    },
    File {
        key: b"B5pc-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/B5pc-V"),
    },
    File {
        key: b"CNS-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/CNS-EUC-H"),
    },
    File {
        key: b"CNS-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/CNS-EUC-V"),
    },
    File {
        key: b"ETen-B5-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETen-B5-H"),
    },
    File {
        key: b"ETen-B5-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETen-B5-V"),
    },
    File {
        key: b"ETenms-B5-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETenms-B5-H"),
    },
    File {
        key: b"ETenms-B5-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETenms-B5-V"),
    },
    File {
        key: b"HKscs-B5-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/HKscs-B5-H"),
    },
    File {
        key: b"HKscs-B5-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/HKscs-B5-V"),
    },
    File {
        key: b"UniCNS-UCS2-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UCS2-H"),
    },
    File {
        key: b"UniCNS-UCS2-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UCS2-V"),
    },
    File {
        key: b"UniCNS-UTF16-H",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UTF16-H"),
    },
    File {
        key: b"UniCNS-UTF16-V",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UTF16-V"),
    },
    File {
        key: b"GB-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GB-EUC-H"),
    },
    File {
        key: b"GB-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GB-EUC-V"),
    },
    File {
        key: b"GBK-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK-EUC-H"),
    },
    File {
        key: b"GBK-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK-EUC-V"),
    },
    File {
        key: b"GBK2K-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK2K-H"),
    },
    File {
        key: b"GBK2K-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK2K-V"),
    },
    File {
        key: b"GBKp-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBKp-EUC-H"),
    },
    File {
        key: b"GBKp-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBKp-EUC-V"),
    },
    File {
        key: b"GBpc-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBpc-EUC-H"),
    },
    File {
        key: b"GBpc-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBpc-EUC-V"),
    },
    File {
        key: b"UniGB-UCS2-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UCS2-H"),
    },
    File {
        key: b"UniGB-UCS2-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UCS2-V"),
    },
    File {
        key: b"UniGB-UTF16-H",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UTF16-H"),
    },
    File {
        key: b"UniGB-UTF16-V",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UTF16-V"),
    },
    File {
        key: b"83pv-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/83pv-RKSJ-H"),
    },
    File {
        key: b"90ms-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90ms-RKSJ-H"),
    },
    File {
        key: b"90ms-RKSJ-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90ms-RKSJ-V"),
    },
    File {
        key: b"90msp-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90msp-RKSJ-H"),
    },
    File {
        key: b"90msp-RKSJ-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90msp-RKSJ-V"),
    },
    File {
        key: b"90pv-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90pv-RKSJ-H"),
    },
    File {
        key: b"Add-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Add-RKSJ-H"),
    },
    File {
        key: b"Add-RKSJ-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Add-RKSJ-V"),
    },
    File {
        key: b"EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/EUC-H"),
    },
    File {
        key: b"EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/EUC-V"),
    },
    File {
        key: b"Ext-RKSJ-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Ext-RKSJ-H"),
    },
    File {
        key: b"Ext-RKSJ-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Ext-RKSJ-V"),
    },
    File {
        key: b"H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/H"),
    },
    File {
        key: b"UniJIS-UCS2-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-H"),
    },
    File {
        key: b"UniJIS-UCS2-HW-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-HW-H"),
    },
    File {
        key: b"UniJIS-UCS2-HW-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-HW-V"),
    },
    File {
        key: b"UniJIS-UCS2-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-V"),
    },
    File {
        key: b"UniJIS-UTF16-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UTF16-H"),
    },
    File {
        key: b"UniJIS-UTF16-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UTF16-V"),
    },
    File {
        key: b"V",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/V"),
    },
    File {
        key: b"KSC-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSC-EUC-H"),
    },
    File {
        key: b"KSC-EUC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSC-EUC-V"),
    },
    File {
        key: b"KSCms-UHC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-H"),
    },
    File {
        key: b"KSCms-UHC-HW-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-HW-H"),
    },
    File {
        key: b"KSCms-UHC-HW-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-HW-V"),
    },
    File {
        key: b"KSCms-UHC-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-V"),
    },
    File {
        key: b"KSCpc-EUC-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCpc-EUC-H"),
    },
    File {
        key: b"UniKS-UCS2-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UCS2-H"),
    },
    File {
        key: b"UniKS-UCS2-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UCS2-V"),
    },
    File {
        key: b"UniKS-UTF16-H",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UTF16-H"),
    },
    File {
        key: b"UniKS-UTF16-V",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UTF16-V"),
    },
];
#[cfg(not(feature = "cmaps"))]
pub(crate) const CMAPS: &[File] = &[];

/// The `cid2unicode.txt` projection of each collection, by ordering.
#[cfg(feature = "cmaps")]
pub(crate) const UNICODE: &[File] = &[
    File {
        key: b"CNS1",
        data: include_bytes!("../../../data/cmaps/Adobe-CNS1-7/cid2unicode.txt"),
    },
    File {
        key: b"GB1",
        data: include_bytes!("../../../data/cmaps/Adobe-GB1-6/cid2unicode.txt"),
    },
    File {
        key: b"Japan1",
        data: include_bytes!("../../../data/cmaps/Adobe-Japan1-7/cid2unicode.txt"),
    },
    File {
        key: b"Korea1",
        data: include_bytes!("../../../data/cmaps/Adobe-Korea1-2/cid2unicode.txt"),
    },
];
#[cfg(not(feature = "cmaps"))]
pub(crate) const UNICODE: &[File] = &[];

#[cfg(all(test, feature = "cmaps"))]
mod tests {
    use crate::{
        Bundled,
        oracle::{names, row, rows, text},
    };
    use pagura_core::{Error, ErrorKind, Limits, Result, Value};
    use pagura_testkit::fixture;
    use pagura_text::{PredefinedCMap, Provider, WritingMode};
    use std::{collections::BTreeSet, sync::Arc};

    fn integer(value: &Value) -> Result<u32> {
        value
            .as_integer()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| Error::syntax("oracle must be an unsigned integer"))
    }

    #[test]
    fn every_predefined_cmap_loads_once_and_matches_its_file_oracles() -> Result<()> {
        let provider = Bundled::default();
        let mut covered = BTreeSet::new();
        for name in names("bundled-cmap-inventory")? {
            let cmap_name = fixture("text", &format!("{name}.cmap"));
            let cmap = provider
                .predefined_cmap(&cmap_name)?
                .ok_or_else(|| Error::syntax("missing predefined CMap"))?;
            let again = provider
                .predefined_cmap(&cmap_name)?
                .ok_or_else(|| Error::syntax("missing cached CMap"))?;
            assert!(PredefinedCMap::ptr_eq(&cmap, &again));
            let mode = fixture("text", &format!("{name}.mode"));
            assert_eq!(
                cmap.writing_mode(),
                if mode == b"1" {
                    WritingMode::Vertical
                } else {
                    WritingMode::Horizontal
                }
            );
            let collection = fixture("text", &format!("{name}.collection"));
            assert_eq!(
                cmap.collection(),
                Some((b"Adobe".as_slice(), collection.as_slice()))
            );
            let input = fixture("text", &format!("{name}.input"));
            let mut bytes = input.as_slice();
            for value in rows(&name, "out")? {
                let values = row(&value)?;
                let decoded = cmap.decode_code(bytes, &Limits::default())?;
                let (code, len, cid) = (decoded.code, decoded.byte_len, u32::from(decoded.cid));
                assert_eq!(
                    (code, u32::from(len), cid),
                    (
                        integer(&values[0])?,
                        integer(&values[1])?,
                        integer(&values[2])?
                    ),
                    "{name}"
                );
                bytes = &bytes[usize::from(len)..];
            }
            assert_eq!(bytes, []);
            covered.insert(cmap_name);
        }
        let table: BTreeSet<_> = super::CMAPS.iter().map(|file| file.key.to_vec()).collect();
        assert_eq!(covered, table);
        Ok(())
    }

    #[test]
    fn collection_unicode_aliases_load_once_and_match_independent_rows() -> Result<()> {
        let provider = Bundled::default();
        for collection in ["CNS1", "GB1", "Japan1", "Korea1"] {
            let mapping = provider
                .cid_to_unicode(b"Adobe", collection.as_bytes())?
                .ok_or_else(|| Error::syntax("missing collection"))?;
            let again = provider
                .cid_to_unicode(b"Adobe", collection.as_bytes())?
                .ok_or_else(|| Error::syntax("missing cached collection"))?;
            assert!(Arc::ptr_eq(&mapping, &again));
            for value in rows(&format!("collection-{collection}"), "out")? {
                let values = row(&value)?;
                assert_eq!(mapping.get(integer(&values[0])?), text(&values[1]));
            }
        }
        Ok(())
    }

    #[test]
    fn resource_inheritance_cannot_redefine_its_adopted_codespace() {
        let provider = Bundled::default();
        let error = PredefinedCMap::parse(
            b"/90ms-RKSJ-H usecmap 1 begincodespacerange <00> <ff> endcodespacerange",
            &Limits::default(),
            |name| {
                provider
                    .predefined_cmap(name)?
                    .ok_or_else(|| Error::syntax("missing parent"))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
    }

    #[test]
    fn every_pinned_cmap_and_table_fits_the_fixed_limits_and_is_shared() -> Result<()> {
        // A provider whose budgets would refuse every file still gets the
        // tables parsed under the fixed limits, the same values every other
        // provider gets.
        let tight = Bundled::new(Limits {
            depth: 0,
            font_work: 0,
            operations: 0,
            font_entries: 0,
            value_nodes: 0,
            decoded_bytes: 0,
            ..Limits::default()
        });
        for file in super::CMAPS {
            let cmap = super::predefined_cmap(file.key)?
                .ok_or_else(|| Error::syntax("missing predefined CMap"))?;
            let shared = tight
                .predefined_cmap(file.key)?
                .ok_or_else(|| Error::syntax("missing shared CMap"))?;
            assert!(PredefinedCMap::ptr_eq(&cmap, &shared));
        }
        for file in super::UNICODE {
            let table = super::cid_to_unicode(b"Adobe", file.key)?
                .ok_or_else(|| Error::syntax("missing collection"))?;
            let shared = tight
                .cid_to_unicode(b"Adobe", file.key)?
                .ok_or_else(|| Error::syntax("missing shared collection"))?;
            assert!(Arc::ptr_eq(&table, &shared));
        }
        Ok(())
    }
}
