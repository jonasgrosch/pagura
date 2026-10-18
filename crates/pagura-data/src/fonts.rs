//! Pinned Core14 metrics and replacement programs, lazily parsed per provider.
use pagura_core::{Limits, Result};
use pagura_font::{FontFormat, FontProgram};
use pagura_text::StandardFont;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Debug)]
pub(crate) struct Fonts {
    limits: Limits,
    cache: Mutex<BTreeMap<&'static [u8], Arc<StandardFont>>>,
}
impl Fonts {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            cache: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) fn get(&self, name: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let Some((name, afm, outline)) = entry(name) else {
            return Ok(None);
        };
        if let Some(value) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return Ok(Some(Arc::clone(value)));
        }
        let metrics = Arc::new(crate::afm::parse(afm, &self.limits)?);
        let format = if matches!(name, b"Symbol" | b"ZapfDingbats") {
            FontFormat::Cff
        } else {
            FontFormat::TrueType
        };
        let program = outline
            .map(|data| FontProgram::parse(data, format, &self.limits).map(Arc::new))
            .transpose()?;
        let value = Arc::new(StandardFont { metrics, program });
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Some(Arc::clone(cache.entry(name).or_insert(value))))
    }
}

type Entry = (&'static [u8], &'static [u8], Option<&'static [u8]>);
fn entry(name: &[u8]) -> Option<Entry> {
    Some(match name {
        b"Courier" => (
            b"Courier",
            include_bytes!("../../../data/afm/Courier.afm"),
            Some(include_bytes!("../../../data/fonts/Cousine-Regular.ttf").as_slice()),
        ),
        b"Courier-Bold" => (
            b"Courier-Bold",
            include_bytes!("../../../data/afm/Courier-Bold.afm"),
            Some(include_bytes!("../../../data/fonts/Cousine-Bold.ttf").as_slice()),
        ),
        b"Courier-Oblique" => (
            b"Courier-Oblique",
            include_bytes!("../../../data/afm/Courier-Oblique.afm"),
            Some(include_bytes!("../../../data/fonts/Cousine-Italic.ttf").as_slice()),
        ),
        b"Courier-BoldOblique" => (
            b"Courier-BoldOblique",
            include_bytes!("../../../data/afm/Courier-BoldOblique.afm"),
            Some(include_bytes!("../../../data/fonts/Cousine-BoldItalic.ttf").as_slice()),
        ),
        b"Helvetica" => (
            b"Helvetica",
            include_bytes!("../../../data/afm/Helvetica.afm"),
            Some(include_bytes!("../../../data/fonts/Arimo-Regular.ttf").as_slice()),
        ),
        b"Helvetica-Bold" => (
            b"Helvetica-Bold",
            include_bytes!("../../../data/afm/Helvetica-Bold.afm"),
            Some(include_bytes!("../../../data/fonts/Arimo-Bold.ttf").as_slice()),
        ),
        b"Helvetica-Oblique" => (
            b"Helvetica-Oblique",
            include_bytes!("../../../data/afm/Helvetica-Oblique.afm"),
            Some(include_bytes!("../../../data/fonts/Arimo-Italic.ttf").as_slice()),
        ),
        b"Helvetica-BoldOblique" => (
            b"Helvetica-BoldOblique",
            include_bytes!("../../../data/afm/Helvetica-BoldOblique.afm"),
            Some(include_bytes!("../../../data/fonts/Arimo-BoldItalic.ttf").as_slice()),
        ),
        b"Times-Roman" => (
            b"Times-Roman",
            include_bytes!("../../../data/afm/Times-Roman.afm"),
            Some(include_bytes!("../../../data/fonts/Tinos-Regular.ttf").as_slice()),
        ),
        b"Times-Bold" => (
            b"Times-Bold",
            include_bytes!("../../../data/afm/Times-Bold.afm"),
            Some(include_bytes!("../../../data/fonts/Tinos-Bold.ttf").as_slice()),
        ),
        b"Times-Italic" => (
            b"Times-Italic",
            include_bytes!("../../../data/afm/Times-Italic.afm"),
            Some(include_bytes!("../../../data/fonts/Tinos-Italic.ttf").as_slice()),
        ),
        b"Times-BoldItalic" => (
            b"Times-BoldItalic",
            include_bytes!("../../../data/afm/Times-BoldItalic.afm"),
            Some(include_bytes!("../../../data/fonts/Tinos-BoldItalic.ttf").as_slice()),
        ),
        b"Symbol" => (
            b"Symbol",
            include_bytes!("../../../data/afm/Symbol.afm"),
            Some(include_bytes!("../../../data/fonts/FoxitSymbol.cff").as_slice()),
        ),
        b"ZapfDingbats" => (
            b"ZapfDingbats",
            include_bytes!("../../../data/afm/ZapfDingbats.afm"),
            Some(include_bytes!("../../../data/fonts/FoxitDingbats.cff").as_slice()),
        ),
        _ => return None,
    })
}
