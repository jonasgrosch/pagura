//! Per-compilation shared font, outline and image storage admission.
use super::{Arc, Compiler, Dictionary, Error, PdfFont, Result, Value};
use pagura_raster::Path;
use std::collections::{BTreeMap, BTreeSet};

/// Retained document resources. Input/dictionary estimates and path-segment
/// estimates are charged separately from transient page/display-list storage.
#[derive(Debug)]
pub(crate) struct FontCache {
    fonts: BTreeMap<pagura_core::ObjectId, Arc<PdfFont>>,
    direct: Vec<(Dictionary, Arc<PdfFont>)>,
    owners: BTreeSet<usize>,
    pub(super) outlines: BTreeMap<(usize, u32), Arc<Path>>,
    pub(crate) bytes: usize,
    font_bytes: usize,
    maximum: usize,
    entries: usize,
}
impl FontCache {
    pub(crate) fn new(limits: pagura_core::Limits) -> Self {
        Self {
            fonts: BTreeMap::new(),
            direct: Vec::new(),
            owners: BTreeSet::new(),
            outlines: BTreeMap::new(),
            bytes: 0,
            font_bytes: 0,
            maximum: limits.document_bytes,
            entries: limits.display_commands,
        }
    }
    fn get(
        &self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
    ) -> Option<Arc<PdfFont>> {
        key.map_or_else(
            || {
                self.direct
                    .iter()
                    .find(|(d, _)| d == dictionary)
                    .map(|(_, f)| Arc::clone(f))
            },
            |id| self.fonts.get(&id).cloned(),
        )
    }
    fn remember(
        &mut self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
        font: Arc<PdfFont>,
        bytes: usize,
    ) -> Arc<PdfFont> {
        // A simultaneous compilation may have published this font while the
        // current load ran outside the cache lock. Keep one immutable identity.
        if let Some(existing) = self.get(key, dictionary) {
            return existing;
        }
        let bytes = bytes.saturating_add(128);
        if bytes > self.maximum || self.entries == 0 {
            return font;
        }
        if !self.fits(bytes) || self.owners.len() >= 256 {
            self.fonts.clear();
            self.direct.clear();
            self.owners.clear();
            self.outlines.clear();
            self.bytes = 0;
            self.font_bytes = 0;
        }
        self.bytes += bytes;
        self.font_bytes += bytes;
        self.owners.insert(Arc::as_ptr(&font) as usize);
        if let Some(id) = key {
            self.fonts.insert(id, Arc::clone(&font));
        } else {
            self.direct.push((dictionary.clone(), Arc::clone(&font)));
        }
        font
    }
    pub(super) fn outline(&mut self, font: &PdfFont, key: (usize, u32), path: Arc<Path>) {
        if !self.owners.contains(&(std::ptr::from_ref(font) as usize))
            || self.outlines.contains_key(&key)
        {
            return;
        }
        let bytes = super::path_bytes(&path).saturating_add(96);
        if !self.fits(bytes) {
            self.outlines.clear();
            self.bytes = self.font_bytes;
        }
        if self.fits(bytes) {
            self.bytes += bytes;
            self.outlines.insert(key, path);
        }
    }
    fn fits(&self, bytes: usize) -> bool {
        self.bytes
            .checked_add(bytes)
            .is_some_and(|n| n <= self.maximum)
            && self.entry_count() < self.entries
    }
    fn entry_count(&self) -> usize {
        self.owners.len().saturating_add(self.outlines.len())
    }
}

#[derive(Debug, Default)]
pub(in crate::interpreter) struct Cache {
    pub(super) fonts: BTreeMap<pagura_core::ObjectId, Arc<PdfFont>>,
    direct_fonts: Vec<(Dictionary, Arc<PdfFont>)>,
    resources: Vec<Dictionary>,
    pub(in crate::interpreter) page_resources: Option<Arc<Dictionary>>,
    pub(super) outlines: BTreeMap<(usize, u32), Arc<Path>>,
    pub(in crate::interpreter) images:
        BTreeMap<(pagura_core::ObjectId, usize), Arc<super::super::images::Image>>,
    pub(super) bytes: usize,
    pub(super) entries: usize,
}
impl Compiler<'_> {
    pub(in crate::interpreter) fn paint_charge(&self, bytes: usize, entries: usize) -> Result<()> {
        let limits = self.document.limits();
        let mut cache = self.state.text.cache.borrow_mut();
        let bytes = cache
            .bytes
            .checked_add(bytes)
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("text and image retained byte budget exceeded"))?;
        let entries = cache
            .entries
            .checked_add(entries)
            .filter(|&n| {
                n.checked_add(self.marks)
                    .is_some_and(|total| total <= limits.display_commands)
            })
            .ok_or(Error::limit("text and image cache entry budget exceeded"))?;
        cache.bytes = bytes;
        cache.entries = entries;
        Ok(())
    }
    pub(in crate::interpreter) fn resource_identity(
        &self,
        resources: &Dictionary,
    ) -> Result<usize> {
        if let Some(key) = self
            .state
            .text
            .cache
            .borrow()
            .resources
            .iter()
            .position(|r| r == resources)
        {
            return Ok(key);
        }
        self.paint_charge(resources.retained_bytes(), 1)?;
        let mut cache = self.state.text.cache.borrow_mut();
        if cache.resources.len() >= 256 {
            return Err(Error::limit("image resource cache budget exceeded"));
        }
        let key = cache.resources.len();
        cache.resources.push(resources.clone());
        Ok(key)
    }
    pub(super) fn select_font(&mut self, value: &Value, size: f64) -> Result<()> {
        let dictionary = self.document.resolve_dictionary(value)?;
        let key = value.as_reference();
        let local = {
            let cache = self.state.text.cache.borrow();
            key.map_or_else(
                || {
                    cache
                        .direct_fonts
                        .iter()
                        .find(|(d, _)| d == dictionary)
                        .map(|(_, font)| Arc::clone(font))
                },
                |id| cache.fonts.get(&id).cloned(),
            )
        };
        let in_page = local.is_some();
        let shared = if in_page {
            None
        } else {
            self.profiles.fonts.and_then(|fonts| {
                fonts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(key, dictionary)
            })
        };
        let shared_hit = shared.is_some();
        let cached = local.or(shared);
        let font = if let Some(font) = cached {
            font
        } else {
            let bytes = dictionary
                .retained_bytes()
                .saturating_add(font_input_bytes(self.document, dictionary, 0)?)
                .saturating_add(size_of::<PdfFont>());
            self.paint_charge(bytes, 1)?;
            let loaded = self.profiles.provider.map_or_else(
                || PdfFont::load(self.document, dictionary),
                |provider| PdfFont::load_with(self.document, dictionary, provider),
            );
            let font = Arc::new(loaded.map_err(|mut error| {
                if matches!(
                    error.position,
                    pagura_core::Position::Unknown | pagura_core::Position::Buffer(_)
                ) {
                    error.position = self.position;
                }
                key.map_or(error.clone(), |id| error.in_object(id))
            })?);
            if let Some(fonts) = self.profiles.fonts {
                fonts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remember(key, dictionary, font, bytes)
            } else {
                font
            }
        };
        // Every compilation owns its selected fonts, even after a document
        // cache eviction. Account local entries on shared-cache hits too.
        if !in_page {
            if shared_hit {
                let bytes = 64_usize.saturating_add(if key.is_none() {
                    dictionary.retained_bytes()
                } else {
                    0
                });
                self.paint_charge(bytes, 1)?;
            }
            let mut cache = self.state.text.cache.borrow_mut();
            if cache.fonts.len() + cache.direct_fonts.len() >= 256 {
                return Err(Error::limit("font cache budget exceeded"));
            }
            if let Some(id) = key {
                cache.fonts.insert(id, Arc::clone(&font));
            } else {
                cache
                    .direct_fonts
                    .push((dictionary.clone(), Arc::clone(&font)));
            }
        }
        self.state.text.font = Some(font);
        self.state.text.size = size;
        Ok(())
    }
    // Text entries are separated before delegating the remaining ExtGState to
    // the existing implementation, preserving its precedence and validation.
    pub(in crate::interpreter) fn paint_gstate(
        &mut self,
        values: &[Value],
        resources: &Dictionary,
    ) -> Result<()> {
        let name = values
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("ExtGState name expected"))?;
        let value = super::resource(self.document, resources, b"ExtGState", name.as_bytes())?;
        let dict = self.document.resolve_dictionary(value)?;
        let uncolored = self.uncolored_glyph();
        if let Some(font) = dict.get(b"Font") {
            let array = self
                .document
                .resolve(font)?
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(Error::syntax("ExtGState Font requires font and size"))?;
            let size = self
                .document
                .resolve(&array[1])?
                .as_number()
                .ok_or(Error::syntax("invalid ExtGState font size"))?;
            self.select_font(&array[0], size)?;
        }
        if let Some(knockout) = dict.get(b"TK") {
            let knockout = self
                .document
                .resolve(knockout)?
                .as_bool()
                .ok_or(Error::syntax("TK must be boolean"))?;
            if self.state.text.object.is_none() && !uncolored {
                self.state.text.knockout = knockout;
            }
        }
        if !dict.contains_key(b"Font") && !uncolored {
            return self.extgstate(values, resources);
        }
        let filtered: Dictionary = dict
            .iter()
            .filter(|(k, _)| *k != b"Font" && !(uncolored && color_parameter(k.as_bytes())))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut states = Dictionary::new();
        states.insert(name.clone(), Value::Dictionary(filtered));
        let mut local = resources.clone();
        local.insert(
            pagura_core::Name::new(b"ExtGState"),
            Value::Dictionary(states),
        );
        self.extgstate(values, &local)
    }
}

fn color_parameter(key: &[u8]) -> bool {
    matches!(
        key,
        b"RI"
            | b"BG"
            | b"BG2"
            | b"UCR"
            | b"UCR2"
            | b"TR"
            | b"TR2"
            | b"HT"
            | b"UseBlackPtComp"
            | b"CA"
            | b"ca"
            | b"BM"
            | b"SMask"
            | b"AIS"
            | b"OP"
            | b"op"
            | b"OPM"
            | b"TK"
    )
}

fn font_input_bytes(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    depth: usize,
) -> Result<usize> {
    if depth >= document.limits().depth {
        return Err(Error::limit("font resource nesting exceeded"));
    }
    let mut bytes = 0_usize;
    if let Some(descriptor) = dictionary.get(b"FontDescriptor") {
        for (key, value) in document.resolve_dictionary(descriptor)?.iter() {
            if matches!(key.as_bytes(), b"FontFile" | b"FontFile2" | b"FontFile3") {
                let id = value
                    .as_reference()
                    .ok_or(Error::syntax("font program must be indirect"))?;
                bytes = bytes
                    .checked_add(document.stream_data(document.object(id)?)?.len())
                    .ok_or(Error::limit("font input byte overflow"))?;
            }
        }
    }
    if dictionary.is_name(b"Subtype", b"Type0")
        && let Some(descendants) = dictionary.get(b"DescendantFonts")
    {
        let descendants = document
            .resolve(descendants)?
            .as_array()
            .filter(|values| values.len() == 1)
            .ok_or(Error::syntax("Type0 requires one descendant font"))?;
        let descendant = document.resolve_dictionary(&descendants[0])?;
        if !descendant.is_name(b"Subtype", b"CIDFontType0")
            && !descendant.is_name(b"Subtype", b"CIDFontType2")
        {
            return Err(Error::syntax("invalid CIDFont subtype"));
        }
        bytes = bytes
            .checked_add(font_input_bytes(document, descendant, depth + 1)?)
            .ok_or(Error::limit("font input byte overflow"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_cache_admission_eviction_and_duplicate_publication_are_bounded() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-mode-0.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let id = pagura_core::ObjectId::new(5, 0);
        let dictionary = document.object(id).unwrap().dictionary().unwrap();
        let font = Arc::new(PdfFont::load(&document, dictionary).unwrap());
        let duplicate = Arc::new(PdfFont::load(&document, dictionary).unwrap());
        let mut disabled = FontCache::new(pagura_core::Limits {
            document_bytes: 0,
            ..Default::default()
        });
        assert!(Arc::ptr_eq(
            &disabled.remember(Some(id), dictionary, Arc::clone(&font), 100),
            &font
        ));
        assert_eq!(disabled.bytes, 0);
        assert!(disabled.fonts.is_empty());
        let mut fonts = FontCache::new(pagura_core::Limits {
            document_bytes: 1024,
            ..Default::default()
        });
        fonts.remember(Some(id), dictionary, Arc::clone(&font), 100);
        let bytes = fonts.bytes;
        assert!(Arc::ptr_eq(
            &fonts.remember(Some(id), dictionary, duplicate, 100),
            &font
        ));
        assert_eq!(fonts.bytes, bytes);
        let mut path = pagura_raster::PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        let path = Arc::new(path.finish());
        for glyph in 0..20 {
            fonts.outline(&font, (123, glyph), Arc::clone(&path));
            assert!(fonts.bytes <= 1024);
        }
        assert!(fonts.outlines.len() < 20);
        let bytes = fonts.bytes;
        fonts.outline(&font, (123, 19), Arc::clone(&path));
        assert_eq!(fonts.bytes, bytes);
        // A new font that needs the whole cache evicts the old font and every
        // pointer-keyed outline together. Active compilations own separate Arcs.
        fonts.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            dictionary,
            Arc::clone(&font),
            850,
        );
        assert!(fonts.get(Some(id), dictionary).is_none());
        assert!(fonts.outlines.is_empty());
        assert!(fonts.bytes <= 1024);
    }
    #[test]
    fn embedded_cid_font_inputs_are_charged_through_descendants() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "bundled-cjk-cmap.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let fonts = document
            .resolve_dictionary(document.page(0).unwrap().resources().get(b"Font").unwrap())
            .unwrap();
        let font = document
            .resolve_dictionary(fonts.get(b"F").unwrap())
            .unwrap();
        let program = document
            .stream_data(document.object(pagura_core::ObjectId::new(8, 0)).unwrap())
            .unwrap();
        assert_eq!(font_input_bytes(&document, font, 0).unwrap(), program.len());
        assert!(!program.is_empty());
    }
    #[test]
    fn malformed_font_fanout_is_rejected_before_loading_descendants() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-mode-0.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let dictionary: Dictionary = [
            (pagura_core::Name::new(b"Subtype"), Value::name(b"Type0")),
            (
                pagura_core::Name::new(b"DescendantFonts"),
                Value::Array(vec![Value::Reference(pagura_core::ObjectId::new(5, 0)); 2]),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            font_input_bytes(&document, &dictionary, 0)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Syntax
        );
    }
}
