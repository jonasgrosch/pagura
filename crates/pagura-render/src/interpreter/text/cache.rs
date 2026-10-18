//! Per-compilation shared font, outline and image storage admission.
use super::{Arc, Compiler, Dictionary, Error, PdfFont, Result, Value};
use pagura_raster::Path;
use std::collections::BTreeMap;

/// One immutable font and its outlines. Active compilations share this record;
/// eviction removes its document admission without reassigning its identity.
#[derive(Debug)]
pub(super) struct CachedFont {
    pub(super) font: Arc<PdfFont>,
    object: Option<pagura_core::ObjectId>,
    outlines: std::sync::Mutex<Outlines>,
}

#[derive(Debug, Default)]
struct Outlines {
    admitted: bool,
    paths: BTreeMap<u32, Arc<Path>>,
}

impl CachedFont {
    fn new(font: Arc<PdfFont>, object: Option<pagura_core::ObjectId>, admitted: bool) -> Self {
        Self {
            font,
            object,
            outlines: std::sync::Mutex::new(Outlines {
                admitted,
                ..Default::default()
            }),
        }
    }

    fn lock_outlines(&self) -> std::sync::MutexGuard<'_, Outlines> {
        self.outlines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn outline(&self, glyph: u32) -> Option<Arc<Path>> {
        self.lock_outlines().paths.get(&glyph).cloned()
    }

    fn remember(&self, glyph: u32, path: Arc<Path>) {
        self.lock_outlines().paths.insert(glyph, path);
    }
}

#[derive(Debug, Default)]
struct Fonts {
    indirect: BTreeMap<pagura_core::ObjectId, Arc<CachedFont>>,
    direct: Vec<(Dictionary, Arc<CachedFont>)>,
}

impl Fonts {
    fn get(
        &self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
    ) -> Option<Arc<CachedFont>> {
        key.map_or_else(
            || {
                self.direct
                    .iter()
                    .find(|(d, _)| d == dictionary)
                    .map(|(_, f)| Arc::clone(f))
            },
            |id| self.indirect.get(&id).cloned(),
        )
    }

    fn insert(
        &mut self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
        font: Arc<CachedFont>,
    ) {
        if let Some(id) = key {
            self.indirect.insert(id, font);
        } else {
            self.direct.push((dictionary.clone(), font));
        }
    }

    fn len(&self) -> usize {
        self.indirect.len() + self.direct.len()
    }

    fn values(&self) -> impl Iterator<Item = &Arc<CachedFont>> {
        self.indirect
            .values()
            .chain(self.direct.iter().map(|(_, font)| font))
    }
}

/// Document resources. Each dictionary owns its font and glyph outlines;
/// eviction releases both together. Active compilations retain their own Arcs.
#[derive(Debug)]
pub(crate) struct FontCache {
    fonts: Fonts,
    pub(crate) bytes: usize,
    font_bytes: usize,
    maximum: usize,
    entries: usize,
    outlines: usize,
}

impl FontCache {
    pub(crate) fn new(limits: pagura_core::Limits) -> Self {
        Self {
            fonts: Fonts::default(),
            bytes: 0,
            font_bytes: 0,
            maximum: limits.document_bytes,
            entries: limits.display_commands,
            outlines: 0,
        }
    }

    fn get(
        &self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
    ) -> Option<Arc<CachedFont>> {
        self.fonts.get(key, dictionary)
    }

    fn remember(
        &mut self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
        font: Arc<PdfFont>,
        bytes: usize,
    ) -> Arc<CachedFont> {
        // Loading happens outside the lock; publish each dictionary only once.
        if let Some(existing) = self.get(key, dictionary) {
            return existing;
        }
        let bytes = bytes.saturating_add(128);
        if bytes > self.maximum || self.entries == 0 {
            return Arc::new(CachedFont::new(font, key, false));
        }
        if !self.fits(bytes) || self.fonts.len() >= 256 {
            for font in self.fonts.values() {
                let mut outlines = font.lock_outlines();
                outlines.paths.clear();
                outlines.admitted = false;
            }
            self.fonts = Fonts::default();
            self.bytes = 0;
            self.font_bytes = 0;
            self.outlines = 0;
        }
        let font = Arc::new(CachedFont::new(font, key, true));
        self.bytes += bytes;
        self.font_bytes += bytes;
        self.fonts.insert(key, dictionary, Arc::clone(&font));
        font
    }

    fn outline(&mut self, font: &CachedFont, glyph: u32, path: Arc<Path>) {
        let mut outlines = font.lock_outlines();
        if outlines.paths.contains_key(&glyph) {
            return;
        }
        if !outlines.admitted {
            // An evicted font can still belong to active compilations. Their
            // transient budgets cover it; it never writes into a replacement.
            outlines.paths.insert(glyph, path);
            return;
        }
        drop(outlines);
        let bytes = super::path_bytes(&path).saturating_add(96);
        if !self.fits(bytes) {
            for font in self.fonts.values() {
                font.lock_outlines().paths.clear();
            }
            self.outlines = 0;
            self.bytes = self.font_bytes;
        }
        if self.fits(bytes) {
            font.remember(glyph, path);
            self.bytes += bytes;
            self.outlines += 1;
        }
    }

    fn fits(&self, bytes: usize) -> bool {
        self.bytes
            .checked_add(bytes)
            .is_some_and(|n| n <= self.maximum)
            && self.fonts.len().saturating_add(self.outlines) < self.entries
    }
}

#[derive(Debug, Default)]
pub(in crate::interpreter) struct Cache {
    fonts: Fonts,
    resources: Vec<Dictionary>,
    pub(in crate::interpreter) page_resources: Option<Arc<Dictionary>>,
    pub(in crate::interpreter) images:
        BTreeMap<(pagura_core::ObjectId, usize), Arc<super::super::images::Image>>,
    pub(super) bytes: usize,
    pub(super) entries: usize,
}

#[cfg(test)]
impl Cache {
    pub(super) fn font_counts(&self) -> (usize, usize) {
        (
            self.fonts.len(),
            self.fonts
                .values()
                .map(|f| f.lock_outlines().paths.len())
                .sum(),
        )
    }
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
        let local = self.state.text.cache.borrow().fonts.get(key, dictionary);
        let in_page = local.is_some();
        let shared = if in_page {
            None
        } else {
            self.compilation.fonts.and_then(|fonts| {
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
            self.load_font(key, dictionary)?
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
            if cache.fonts.len() >= 256 {
                return Err(Error::limit("font cache budget exceeded"));
            }
            cache.fonts.insert(key, dictionary, Arc::clone(&font));
        }
        self.state.text.font = Some(font);
        self.state.text.size = size;
        Ok(())
    }

    fn load_font(
        &self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
    ) -> Result<Arc<CachedFont>> {
        let bytes = dictionary
            .retained_bytes()
            .saturating_add(font_input_bytes(self.document, dictionary, 0)?)
            .saturating_add(size_of::<PdfFont>());
        self.paint_charge(bytes, 1)?;
        let loaded = self.compilation.provider.map_or_else(
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
            if let Some(id) = key {
                error.in_object(id)
            } else {
                error
            }
        })?);
        Ok(if let Some(fonts) = self.compilation.fonts {
            fonts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remember(key, dictionary, font, bytes)
        } else {
            Arc::new(CachedFont::new(font, key, false))
        })
    }

    pub(super) fn glyph_outline(
        &self,
        font: &CachedFont,
        glyph: pagura_font::GlyphId,
        program: &pagura_font::FontProgram,
    ) -> Result<Arc<Path>> {
        if let Some(path) = font.outline(glyph.0) {
            return Ok(path);
        }
        let path = {
            let mut diagnostics = pagura_core::Diagnostics::new(self.document.recovery());
            let result = program.outline_with_diagnostics(glyph, &mut diagnostics);
            self.document
                .report(diagnostics.take().into_iter().map(|mut diagnostic| {
                    diagnostic.position = self.position;
                    diagnostic.object = font.object;
                    diagnostic
                }));
            Arc::new(result.map_err(|mut error| {
                error.position = self.position;
                error
            })?)
        };
        self.paint_charge(super::path_bytes(&path), 1)?;
        if let Some(fonts) = self.compilation.fonts {
            fonts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .outline(font, glyph.0, Arc::clone(&path));
        } else {
            font.remember(glyph.0, Arc::clone(&path));
        }
        Ok(path)
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
    fn font() -> Result<(Dictionary, Arc<PdfFont>)> {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-mode-0.pdf"),
            &pagura_document::OpenOptions::default(),
        )?;
        let dictionary = document
            .object(pagura_core::ObjectId::new(5, 0))?
            .dictionary()
            .ok_or(Error::syntax("missing font dictionary"))?
            .clone();
        let font = Arc::new(PdfFont::load(&document, &dictionary)?);
        Ok((dictionary, font))
    }

    #[test]
    fn retained_cache_admission_eviction_and_duplicate_publication_are_bounded() {
        let (dictionary, font) = font().unwrap();
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let mut disabled = FontCache::new(pagura_core::Limits {
            document_bytes: 0,
            ..Default::default()
        });
        assert!(Arc::ptr_eq(
            &disabled
                .remember(id, &dictionary, Arc::clone(&font), 100)
                .font,
            &font
        ));
        assert_eq!(disabled.bytes, 0);
        assert_eq!(disabled.fonts.len(), 0);
        let mut fonts = FontCache::new(pagura_core::Limits {
            document_bytes: 1024,
            ..Default::default()
        });
        let first = fonts.remember(id, &dictionary, Arc::clone(&font), 100);
        let bytes = fonts.bytes;
        let same = fonts.remember(id, &dictionary, Arc::clone(&font), 100);
        assert!(Arc::ptr_eq(&first, &same));
        assert_eq!(fonts.bytes, bytes);
        let mut path = pagura_raster::PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        let path = Arc::new(path.finish());
        for glyph in 0..20 {
            fonts.outline(&first, glyph, Arc::clone(&path));
            assert!(fonts.bytes <= 1024);
        }
        assert!(fonts.outlines < 20);
        let bytes = fonts.bytes;
        fonts.outline(&first, 19, Arc::clone(&path));
        assert_eq!(fonts.bytes, bytes);
        fonts.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &dictionary,
            font,
            850,
        );
        assert!(fonts.get(id, &dictionary).is_none());
        assert_eq!(fonts.outlines, 0);
        assert!(first.outline(19).is_none());
        assert!(fonts.bytes <= 1024);
    }

    #[test]
    fn each_dictionary_owns_its_glyph_outlines() {
        let (dictionary, font) = font().unwrap();
        let mut cache = FontCache::new(pagura_core::Limits::default());
        let id = Some(pagura_core::ObjectId::new(5, 0));
        for (key, width) in [(id, 1.0), (None, 2.0)] {
            let font = cache.remember(key, &dictionary, Arc::clone(&font), 100);
            let mut path = pagura_raster::PathBuilder::new();
            path.rect(0.0, 0.0, width, 1.0);
            cache.outline(&font, 1, Arc::new(path.finish()));
        }
        let first = cache.get(id, &dictionary).unwrap().outline(1).unwrap();
        let second = cache.get(None, &dictionary).unwrap().outline(1).unwrap();
        assert_ne!(first, second);
        assert_eq!(cache.outlines, 2);
    }

    #[test]
    fn evicted_records_cannot_populate_replacements() {
        let (dictionary, font) = font().unwrap();
        let mut cache = FontCache::new(pagura_core::Limits {
            document_bytes: 1024,
            ..Default::default()
        });
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let old = cache.remember(id, &dictionary, Arc::clone(&font), 700);
        cache.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &dictionary,
            Arc::clone(&font),
            700,
        );
        let replacement = cache.remember(id, &dictionary, font, 700);
        let bytes = cache.bytes;
        let mut path = pagura_raster::PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        cache.outline(&old, 1, Arc::new(path.finish()));
        assert!(old.outline(1).is_some());
        assert!(replacement.outline(1).is_none());
        assert_eq!(cache.bytes, bytes);
        assert_eq!(cache.outlines, 0);
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
