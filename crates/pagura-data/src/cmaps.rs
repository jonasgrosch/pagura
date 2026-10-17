//! Pinned Table 116 CMaps and collection Unicode tables.
use pagura_core::{Error, Limits, Result};
use pagura_text::{CidToUnicode, PredefinedCMap};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Debug)]
pub(crate) struct CMaps {
    limits: Limits,
    maps: Mutex<BTreeMap<&'static [u8], Arc<PredefinedCMap>>>,
    unicode: Mutex<BTreeMap<&'static [u8], Arc<CidToUnicode>>>,
}
impl CMaps {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            maps: Mutex::new(BTreeMap::new()),
            unicode: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) fn get(&self, name: &[u8]) -> Result<Option<Arc<PredefinedCMap>>> {
        self.load(name, &mut Vec::new())
    }
    fn load(
        &self,
        name: &[u8],
        path: &mut Vec<&'static [u8]>,
    ) -> Result<Option<Arc<PredefinedCMap>>> {
        let Some((name, data)) = map_entry(name) else {
            return Ok(None);
        };
        if path.contains(&name) {
            return Err(Error::cycle("predefined usecmap cycle"));
        }
        if path.len() >= self.limits.depth {
            return Err(Error::limit("predefined CMap depth exceeded"));
        }
        if let Some(value) = self
            .maps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return Ok(Some(Arc::clone(value)));
        }
        path.push(name);
        // Parse outside locks: parents recurse and concurrent readers can race
        // harmlessly. Only one immutable result is retained for each known key.
        let value = Arc::new(PredefinedCMap::parse(data, &self.limits, |parent| {
            self.load(parent, path)?
                .ok_or_else(|| Error::syntax("missing predefined usecmap parent"))
        })?);
        path.pop();
        let mut cache = self.maps.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Some(Arc::clone(cache.entry(name).or_insert(value))))
    }
    pub(crate) fn unicode(
        &self,
        registry: &[u8],
        ordering: &[u8],
    ) -> Result<Option<Arc<CidToUnicode>>> {
        if registry != b"Adobe" {
            return Ok(None);
        }
        let Some((ordering, data)) = unicode_entry(ordering) else {
            return Ok(None);
        };
        if let Some(value) = self
            .unicode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(ordering)
        {
            return Ok(Some(Arc::clone(value)));
        }
        let value = Arc::new(crate::unicode::parse(data, self.limits)?);
        let mut cache = self.unicode.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Some(Arc::clone(cache.entry(ordering).or_insert(value))))
    }
}

fn map_entry(name: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    MAPS.iter().find(|(key, _)| *key == name).copied()
}

const MAPS: &[(&[u8], &[u8])] = &[
    (
        b"B5pc-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/B5pc-H"),
    ),
    (
        b"B5pc-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/B5pc-V"),
    ),
    (
        b"CNS-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/CNS-EUC-H"),
    ),
    (
        b"CNS-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/CNS-EUC-V"),
    ),
    (
        b"ETen-B5-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETen-B5-H"),
    ),
    (
        b"ETen-B5-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETen-B5-V"),
    ),
    (
        b"ETenms-B5-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETenms-B5-H"),
    ),
    (
        b"ETenms-B5-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/ETenms-B5-V"),
    ),
    (
        b"HKscs-B5-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/HKscs-B5-H"),
    ),
    (
        b"HKscs-B5-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/HKscs-B5-V"),
    ),
    (
        b"UniCNS-UCS2-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UCS2-H"),
    ),
    (
        b"UniCNS-UCS2-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UCS2-V"),
    ),
    (
        b"UniCNS-UTF16-H",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UTF16-H"),
    ),
    (
        b"UniCNS-UTF16-V",
        include_bytes!("../../../data/cmaps/Adobe-CNS1-7/UniCNS-UTF16-V"),
    ),
    (
        b"GB-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GB-EUC-H"),
    ),
    (
        b"GB-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GB-EUC-V"),
    ),
    (
        b"GBK-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK-EUC-H"),
    ),
    (
        b"GBK-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK-EUC-V"),
    ),
    (
        b"GBK2K-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK2K-H"),
    ),
    (
        b"GBK2K-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBK2K-V"),
    ),
    (
        b"GBKp-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBKp-EUC-H"),
    ),
    (
        b"GBKp-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBKp-EUC-V"),
    ),
    (
        b"GBpc-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBpc-EUC-H"),
    ),
    (
        b"GBpc-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/GBpc-EUC-V"),
    ),
    (
        b"UniGB-UCS2-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UCS2-H"),
    ),
    (
        b"UniGB-UCS2-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UCS2-V"),
    ),
    (
        b"UniGB-UTF16-H",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UTF16-H"),
    ),
    (
        b"UniGB-UTF16-V",
        include_bytes!("../../../data/cmaps/Adobe-GB1-6/UniGB-UTF16-V"),
    ),
    (
        b"83pv-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/83pv-RKSJ-H"),
    ),
    (
        b"90ms-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90ms-RKSJ-H"),
    ),
    (
        b"90ms-RKSJ-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90ms-RKSJ-V"),
    ),
    (
        b"90msp-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90msp-RKSJ-H"),
    ),
    (
        b"90msp-RKSJ-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90msp-RKSJ-V"),
    ),
    (
        b"90pv-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/90pv-RKSJ-H"),
    ),
    (
        b"Add-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Add-RKSJ-H"),
    ),
    (
        b"Add-RKSJ-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Add-RKSJ-V"),
    ),
    (
        b"EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/EUC-H"),
    ),
    (
        b"EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/EUC-V"),
    ),
    (
        b"Ext-RKSJ-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Ext-RKSJ-H"),
    ),
    (
        b"Ext-RKSJ-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/Ext-RKSJ-V"),
    ),
    (b"H", include_bytes!("../../../data/cmaps/Adobe-Japan1-7/H")),
    (
        b"UniJIS-UCS2-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-H"),
    ),
    (
        b"UniJIS-UCS2-HW-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-HW-H"),
    ),
    (
        b"UniJIS-UCS2-HW-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-HW-V"),
    ),
    (
        b"UniJIS-UCS2-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UCS2-V"),
    ),
    (
        b"UniJIS-UTF16-H",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UTF16-H"),
    ),
    (
        b"UniJIS-UTF16-V",
        include_bytes!("../../../data/cmaps/Adobe-Japan1-7/UniJIS-UTF16-V"),
    ),
    (b"V", include_bytes!("../../../data/cmaps/Adobe-Japan1-7/V")),
    (
        b"KSC-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSC-EUC-H"),
    ),
    (
        b"KSC-EUC-V",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSC-EUC-V"),
    ),
    (
        b"KSCms-UHC-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-H"),
    ),
    (
        b"KSCms-UHC-HW-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-HW-H"),
    ),
    (
        b"KSCms-UHC-HW-V",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-HW-V"),
    ),
    (
        b"KSCms-UHC-V",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCms-UHC-V"),
    ),
    (
        b"KSCpc-EUC-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/KSCpc-EUC-H"),
    ),
    (
        b"UniKS-UCS2-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UCS2-H"),
    ),
    (
        b"UniKS-UCS2-V",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UCS2-V"),
    ),
    (
        b"UniKS-UTF16-H",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UTF16-H"),
    ),
    (
        b"UniKS-UTF16-V",
        include_bytes!("../../../data/cmaps/Adobe-Korea1-2/UniKS-UTF16-V"),
    ),
];

fn unicode_entry(ordering: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    Some(match ordering {
        b"CNS1" => (
            b"CNS1",
            include_bytes!("../../../data/cmaps/Adobe-CNS1-7/cid2unicode.txt"),
        ),
        b"GB1" => (
            b"GB1",
            include_bytes!("../../../data/cmaps/Adobe-GB1-6/cid2unicode.txt"),
        ),
        b"Japan1" => (
            b"Japan1",
            include_bytes!("../../../data/cmaps/Adobe-Japan1-7/cid2unicode.txt"),
        ),
        b"Korea1" => (
            b"Korea1",
            include_bytes!("../../../data/cmaps/Adobe-Korea1-2/cid2unicode.txt"),
        ),
        _ => return None,
    })
}
