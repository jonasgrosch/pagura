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
    /// Whether the program was read only as far as drawing needs it
    /// ([`PdfFont::load_for_rendering`]); only such compilations reuse it.
    drawing: bool,
}

#[derive(Debug, Default)]
struct Outlines {
    admitted: bool,
    paths: BTreeMap<u32, Arc<Path>>,
}

impl CachedFont {
    fn new(
        font: Arc<PdfFont>,
        object: Option<pagura_core::ObjectId>,
        (admitted, drawing): (bool, bool),
    ) -> Self {
        Self {
            font,
            object,
            outlines: std::sync::Mutex::new(Outlines {
                admitted,
                ..Default::default()
            }),
            drawing,
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

    /// The shared font for `dictionary`; one read only for drawing serves
    /// only `drawing` compilations.
    fn get(
        &self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
        drawing: bool,
    ) -> Option<Arc<CachedFont>> {
        self.fonts
            .get(key, dictionary)
            .filter(|font| drawing || !font.drawing)
    }

    fn remember(
        &mut self,
        key: Option<pagura_core::ObjectId>,
        dictionary: &Dictionary,
        (font, drawing): (Arc<PdfFont>, bool),
    ) -> Arc<CachedFont> {
        // Loading happens outside the lock; publish each dictionary only once.
        // A complete program loaded after one read for drawing stays private.
        if let Some(existing) = self.fonts.get(key, dictionary) {
            if drawing || !existing.drawing {
                return existing;
            }
            return Arc::new(CachedFont::new(font, key, (false, drawing)));
        }
        let bytes = font_bytes(&font, key, dictionary);
        if bytes > self.maximum || self.entries == 0 {
            return Arc::new(CachedFont::new(font, key, (false, drawing)));
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
        let font = Arc::new(CachedFont::new(font, key, (true, drawing)));
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
        BTreeMap<(pagura_core::ObjectId, usize, u8), Arc<super::super::images::Image>>,
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
        if self.extraction.is_some() {
            self.select_font_mode::<true>(value, size)
        } else {
            self.select_font_mode::<false>(value, size)
        }
    }

    pub(super) fn select_font_mode<const EXTRACT: bool>(
        &mut self,
        value: &Value,
        size: f64,
    ) -> Result<()> {
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
                    .get(key, dictionary, self.drawing)
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
        if EXTRACT {
            self.extract_font(&font, key, dictionary)?;
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
        let font = cached_font(
            self.document,
            key,
            dictionary,
            self.compilation.provider,
            self.compilation.fonts,
            (self.position, self.drawing),
        )?;
        self.paint_charge(font_bytes(&font.font, key, dictionary), 1)?;
        Ok(font)
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

/// Resolve the same document-bound font entry used by painting.
pub(crate) fn pdf_font(
    document: &pagura_document::Document,
    value: &Value,
    provider: Option<&dyn pagura_text::Provider>,
    fonts: Option<&std::sync::Mutex<FontCache>>,
) -> Result<Arc<PdfFont>> {
    let dictionary = document.resolve_dictionary(value)?;
    let entry = cached_font(
        document,
        value.as_reference(),
        dictionary,
        provider,
        fonts,
        (pagura_core::Position::Unknown, false),
    )?;
    Ok(Arc::clone(&entry.font))
}

fn cached_font(
    document: &pagura_document::Document,
    key: Option<pagura_core::ObjectId>,
    dictionary: &Dictionary,
    provider: Option<&dyn pagura_text::Provider>,
    fonts: Option<&std::sync::Mutex<FontCache>>,
    (position, rendering): (pagura_core::Position, bool),
) -> Result<Arc<CachedFont>> {
    if let Some(entry) = fonts.and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key, dictionary, rendering)
    }) {
        return Ok(entry);
    }
    // Parsing and provider calls happen outside the shared cache lock.
    // A competitive compilation reads a TrueType program only as far as
    // drawing needs ([`PdfFont::load_for_rendering`]).
    let loaded = if rendering {
        PdfFont::load_for_rendering(document, dictionary, provider, position)
    } else {
        PdfFont::load_at(document, dictionary, provider, position)
    };
    let font = Arc::new(loaded.map_err(|mut error| {
        if matches!(
            error.position,
            pagura_core::Position::Unknown | pagura_core::Position::Buffer(_)
        ) {
            error.position = position;
        }
        if error.repair == Some(pagura_text::repairs::DESCRIPTOR_FLAGS) {
            error
        } else if let Some(id) = key {
            error.in_object(id)
        } else {
            error
        }
    })?);
    Ok(match fonts {
        Some(cache) => cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remember(key, dictionary, (font, rendering)),
        None => Arc::new(CachedFont::new(font, key, (false, rendering))),
    })
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

// PdfFont includes its own representation and owned data. Only renderer-owned
// records, Arc counters, tree bookkeeping and direct dictionary copies add to it.
fn font_bytes(
    font: &PdfFont,
    key: Option<pagura_core::ObjectId>,
    dictionary: &Dictionary,
) -> usize {
    font.retained_bytes()
        .saturating_add(size_of::<CachedFont>() + 4 * size_of::<usize>() + 64)
        .saturating_add(size_of::<Arc<CachedFont>>())
        .saturating_add(key.map_or_else(
            || dictionary.retained_bytes(),
            |_| size_of::<pagura_core::ObjectId>(),
        ))
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
                .remember(id, &dictionary, (Arc::clone(&font), false))
                .font,
            &font
        ));
        assert_eq!(disabled.bytes, 0);
        assert_eq!(disabled.fonts.len(), 0);
        let mut fonts = FontCache::new(pagura_core::Limits {
            document_bytes: font_bytes(&font, id, &dictionary) + 1024,
            ..Default::default()
        });
        let first = fonts.remember(id, &dictionary, (Arc::clone(&font), false));
        let bytes = fonts.bytes;
        assert_eq!(bytes, font_bytes(&font, id, &dictionary));
        let same = fonts.remember(id, &dictionary, (Arc::clone(&font), false));
        assert!(Arc::ptr_eq(&first, &same));
        assert_eq!(fonts.bytes, bytes);
        let mut path = pagura_raster::PathBuilder::new();
        path.rect(0.0, 0.0, 2.0, 2.0);
        let path = Arc::new(path.finish());
        for glyph in 0..20 {
            fonts.outline(&first, glyph, Arc::clone(&path));
            assert!(fonts.bytes <= fonts.maximum);
        }
        assert!(fonts.outlines < 20);
        let bytes = fonts.bytes;
        fonts.outline(&first, 19, Arc::clone(&path));
        assert_eq!(fonts.bytes, bytes);
        fonts.maximum = bytes;
        fonts.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &dictionary,
            (font, false),
        );
        assert!(fonts.get(id, &dictionary, false).is_none());
        assert_eq!(fonts.outlines, 0);
        assert!(first.outline(19).is_none());
        assert!(fonts.bytes <= fonts.maximum);
    }

    #[test]
    fn fonts_read_for_drawing_serve_only_drawing_compilations() {
        let (dictionary, font) = font().unwrap();
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let mut cache = FontCache::new(pagura_core::Limits::default());
        let drawing = cache.remember(id, &dictionary, (Arc::clone(&font), true));
        assert!(cache.get(id, &dictionary, false).is_none());
        assert!(Arc::ptr_eq(
            &cache.get(id, &dictionary, true).unwrap(),
            &drawing
        ));
        let whole = cache.remember(id, &dictionary, (Arc::clone(&font), false));
        assert!(!whole.drawing && !Arc::ptr_eq(&whole, &drawing));
        assert!(cache.get(id, &dictionary, false).is_none());
        let mut complete = FontCache::new(pagura_core::Limits::default());
        let whole = complete.remember(id, &dictionary, (Arc::clone(&font), false));
        assert!(Arc::ptr_eq(
            &complete.get(id, &dictionary, true).unwrap(),
            &whole
        ));
        assert!(Arc::ptr_eq(
            &complete.remember(id, &dictionary, (font, true)),
            &whole
        ));
    }

    #[test]
    fn each_dictionary_owns_its_glyph_outlines() {
        let (dictionary, font) = font().unwrap();
        let mut cache = FontCache::new(pagura_core::Limits::default());
        let id = Some(pagura_core::ObjectId::new(5, 0));
        for (key, width) in [(id, 1.0), (None, 2.0)] {
            let font = cache.remember(key, &dictionary, (Arc::clone(&font), false));
            let mut path = pagura_raster::PathBuilder::new();
            path.rect(0.0, 0.0, width, 1.0);
            cache.outline(&font, 1, Arc::new(path.finish()));
        }
        let first = cache
            .get(id, &dictionary, false)
            .unwrap()
            .outline(1)
            .unwrap();
        let second = cache
            .get(None, &dictionary, false)
            .unwrap()
            .outline(1)
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(cache.outlines, 2);
    }

    #[test]
    fn evicted_records_cannot_populate_replacements() {
        let (dictionary, font) = font().unwrap();
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let mut cache = FontCache::new(pagura_core::Limits {
            document_bytes: font_bytes(&font, id, &dictionary),
            ..Default::default()
        });
        let old = cache.remember(id, &dictionary, (Arc::clone(&font), false));
        cache.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &dictionary,
            (Arc::clone(&font), false),
        );
        let replacement = cache.remember(id, &dictionary, (font, false));
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
    fn admission_follows_the_loaded_fonts_owned_estimate() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-mode-0.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let (dictionary, small) = font().unwrap();
        let mut expanded = dictionary.clone();
        expanded.insert(pagura_core::Name::new(b"FirstChar"), Value::Integer(0));
        expanded.insert(pagura_core::Name::new(b"LastChar"), Value::Integer(255));
        expanded.insert(
            pagura_core::Name::new(b"Widths"),
            Value::Array(vec![Value::Integer(400); 256]),
        );
        // A simple font keeps one width per code, so its glyph names make
        // the larger font.
        let mut differences = vec![Value::Integer(0)];
        differences.extend(
            (0..256).map(|code| {
                Value::Name(pagura_core::Name::new(format!("glyph{code:04}").as_bytes()))
            }),
        );
        let mut encoding = Dictionary::new();
        encoding.insert(
            pagura_core::Name::new(b"Differences"),
            Value::Array(differences),
        );
        expanded.insert(
            pagura_core::Name::new(b"Encoding"),
            Value::Dictionary(encoding),
        );
        let large = Arc::new(PdfFont::load(&document, &expanded).unwrap());
        assert!(large.retained_bytes() > small.retained_bytes());
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let mut measured = FontCache::new(pagura_core::Limits::default());
        measured.remember(id, &dictionary, (Arc::clone(&small), false));
        let small_charge = measured.bytes;
        measured.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &expanded,
            (Arc::clone(&large), false),
        );
        let large_charge = measured.bytes - small_charge;
        assert_eq!(
            large_charge - small_charge,
            large.retained_bytes() - small.retained_bytes()
        );
        let mut cache = FontCache::new(pagura_core::Limits {
            document_bytes: small_charge,
            ..Default::default()
        });
        cache.remember(id, &dictionary, (Arc::clone(&small), false));
        assert_eq!(cache.bytes, small_charge);
        cache.remember(
            Some(pagura_core::ObjectId::new(6, 0)),
            &expanded,
            (large, false),
        );
        assert_eq!(cache.fonts.len(), 1);
        assert_eq!(cache.bytes, small_charge);
        let mut direct = FontCache::new(pagura_core::Limits::default());
        direct.remember(None, &dictionary, (Arc::clone(&small), false));
        assert_eq!(
            direct.bytes - small_charge,
            dictionary.retained_bytes() - size_of::<pagura_core::ObjectId>()
        );
    }
    #[test]
    fn embedded_cid_fonts_charge_the_loaded_estimate() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-vertical.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let dictionary = document
            .object(pagura_core::ObjectId::new(5, 0))
            .unwrap()
            .dictionary()
            .unwrap();
        let font = Arc::new(PdfFont::load(&document, dictionary).unwrap());
        let (_, simple) = self::font().unwrap();
        let id = Some(pagura_core::ObjectId::new(5, 0));
        let metadata = font_bytes(&simple, id, dictionary) - simple.retained_bytes();
        let expected = font.retained_bytes() + metadata;
        let mut cache = FontCache::new(pagura_core::Limits::default());
        cache.remember(id, dictionary, (font, false));
        assert_eq!(cache.bytes, expected);
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
            PdfFont::load(&document, &dictionary).unwrap_err().kind,
            pagura_core::ErrorKind::Syntax
        );
    }
}
