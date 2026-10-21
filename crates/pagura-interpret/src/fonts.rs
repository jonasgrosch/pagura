//! Fonts and glyph outlines for one interpretation (9.6–9.7): loaded once
//! per font dictionary, and shared across pages through the `Store` by
//! object identity.

use crate::device::{FontInfo, FontKey, OutlineKey};
use pagura_core::{
    Built, Diagnostics, Dictionary, Error, Meter, ObjectId, Position, Rect, Result, Store,
    StoreKey, Value,
};
use pagura_document::Document;
use pagura_raster::{Path, Segment};
use pagura_text::{FontKind, PdfFont, Provider, WritingMode};
use crate::hash::FastMap;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

/// The most fonts one interpretation selects.
const MAX_FONTS: usize = 256;

/// A font as one interpretation uses it.
#[derive(Debug)]
pub(crate) struct LoadedFont {
    pub(crate) font: Arc<PdfFont>,
    pub(crate) info: FontInfo,
    /// `FontBBox`, which sizes text selection quads, when it can be read;
    /// read only for interpretations that extract text.
    pub(crate) bbox: Option<Rect>,
}

impl LoadedFont {
    pub(crate) fn vertical(&self) -> bool {
        self.font.writing_mode() == WritingMode::Vertical
    }
}

/// The store key of a font program: its dictionary's object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FontStoreKey(ObjectId);

impl StoreKey for FontStoreKey {
    type Value = PdfFont;
}

/// The store key of a glyph outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OutlineStoreKey(OutlineKey);

impl StoreKey for OutlineStoreKey {
    type Value = Path;
}

/// The fonts and outlines of one interpretation.
pub(crate) struct Fonts<'a> {
    document: &'a Document,
    provider: Option<&'a dyn Provider>,
    store: Option<&'a Store>,
    indirect: BTreeMap<ObjectId, Arc<LoadedFont>>,
    direct: Vec<(Dictionary, Arc<LoadedFont>)>,
    outlines: FastMap<OutlineKey, Arc<Path>>,
    bytes: Meter,
    /// Whether text is extracted, which reads font boxes.
    text: bool,
}

impl std::fmt::Debug for Fonts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("fonts", &(self.indirect.len() + self.direct.len()))
            .field("outlines", &self.outlines.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Fonts<'a> {
    pub(crate) fn new(
        document: &'a Document,
        provider: Option<&'a dyn Provider>,
        store: Option<&'a Store>,
        text: bool,
    ) -> Self {
        Self {
            document,
            provider,
            store,
            indirect: BTreeMap::new(),
            direct: Vec::new(),
            outlines: FastMap::default(),
            bytes: Meter::new(
                document.limits().render_bytes,
                "text and image retained byte budget exceeded",
            ),
            text,
        }
    }

    /// The provider fonts the document does not embed come from.
    pub(crate) fn provider(&self) -> Option<&'a dyn Provider> {
        self.provider
    }

    /// The font `value` names, loaded on first use. `position` is the
    /// operator that selects it.
    ///
    /// # Errors
    ///
    /// The errors of loading the font, and `Limit` errors for the font count
    /// and retained bytes.
    pub(crate) fn select(&mut self, value: &Value, position: Position) -> Result<Arc<LoadedFont>> {
        let document = self.document;
        let dictionary = document.resolve_dictionary(value)?;
        let id = value.as_reference();
        let known = match id {
            Some(id) => self.indirect.get(&id).cloned(),
            None => self
                .direct
                .iter()
                .find(|(d, _)| d == dictionary)
                .map(|(_, f)| Arc::clone(f)),
        };
        if let Some(font) = known {
            return Ok(font);
        }
        if self.indirect.len() + self.direct.len() >= MAX_FONTS {
            return Err(Error::limit("font cache budget exceeded"));
        }
        let font = self.load(id, dictionary, position)?;
        let charge = font
            .retained_bytes()
            .saturating_add(size_of::<LoadedFont>() + 64)
            .saturating_add(id.map_or_else(|| dictionary.retained_bytes(), |_| 0));
        self.bytes.charge(charge)?;
        let loaded = Arc::new(LoadedFont {
            info: FontInfo {
                key: font_key(id, dictionary),
                object: id,
                base_name: dictionary
                    .get(b"BaseFont")
                    .and_then(|v| document.resolve(v).ok())
                    .and_then(Value::as_name)
                    .cloned(),
                vertical: font.writing_mode() == WritingMode::Vertical,
                kind: font.kind(),
            },
            bbox: if self.text {
                font_box(document, dictionary, font.kind())
            } else {
                None
            },
            font,
        });
        match id {
            Some(id) => {
                self.indirect.insert(id, Arc::clone(&loaded));
            }
            None => self.direct.push((dictionary.clone(), Arc::clone(&loaded))),
        }
        Ok(loaded)
    }

    /// Loads a font program, through the store for indirect fonts.
    pub(crate) fn load(
        &self,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
        position: Position,
    ) -> Result<Arc<PdfFont>> {
        let load = || -> Result<Built<PdfFont>> {
            let font = PdfFont::load_at(self.document, dictionary, self.provider, position)
                .map_err(|mut error| {
                    if matches!(error.position, Position::Unknown | Position::Buffer(_)) {
                        error.position = position;
                    }
                    if error.repair == Some(pagura_text::repairs::DESCRIPTOR_FLAGS) {
                        error
                    } else {
                        error.in_object_opt(id)
                    }
                })?;
            let bytes = font.retained_bytes();
            Ok(Built { value: font, bytes })
        };
        match (self.store, id) {
            (Some(store), Some(id)) => store.get_or_build(FontStoreKey(id), load),
            _ => load().map(|built| Arc::new(built.value)),
        }
    }

    /// The outline of `glyph` in `program`, read once per [`OutlineKey`]
    /// (font key, glyph) and shared through the store for indirect fonts.
    /// This is the only place an interpretation reads glyph outlines, at the
    /// glyph's first use in source order, so a font that decodes its glyphs
    /// lazily reports them here without other changes. Repairs are reported
    /// at `position` in the font's object.
    ///
    /// # Errors
    ///
    /// The errors of the glyph's charstrings or glyph data, at `position`,
    /// and `Limit` for retained bytes.
    pub(crate) fn outline(
        &mut self,
        font: &LoadedFont,
        glyph: pagura_font::GlyphId,
        program: &pagura_font::FontProgram,
        position: Position,
    ) -> Result<Arc<Path>> {
        let key = OutlineKey {
            font: font.info.key,
            glyph: glyph.0,
        };
        if let Some(path) = self.outlines.get(&key) {
            return Ok(Arc::clone(path));
        }
        let document = self.document;
        let object = font.info.object;
        let read = || -> Result<Built<Path>> {
            let mut diagnostics = Diagnostics::new(document.recovery());
            let result = program.outline_with_diagnostics(glyph, &mut diagnostics);
            document.report(diagnostics.take().into_iter().map(|mut diagnostic| {
                diagnostic.position = position;
                diagnostic.object = object;
                diagnostic
            }));
            let path = result.map_err(|mut error| {
                error.position = position;
                error
            })?;
            let bytes = path_bytes(&path);
            Ok(Built { value: path, bytes })
        };
        let path = match (self.store, object) {
            (Some(store), Some(_)) => store.get_or_build(OutlineStoreKey(key), read)?,
            _ => Arc::new(read()?.value),
        };
        self.bytes.charge(path_bytes(&path))?;
        self.outlines.insert(key, Arc::clone(&path));
        Ok(path)
    }
}

/// The bytes a path's segments occupy.
pub(crate) fn path_bytes(path: &Path) -> usize {
    path.segments().len().saturating_mul(size_of::<Segment>())
}

/// A key that identifies a font within its document: the object of an
/// indirect font, else a hash of the direct dictionary with the high bit
/// set, so the two never meet.
fn font_key(id: Option<ObjectId>, dictionary: &Dictionary) -> FontKey {
    if let Some(id) = id {
        return FontKey((u64::from(id.number) << 16) | u64::from(id.generation));
    }
    let mut hasher = DefaultHasher::new();
    hash_dictionary(dictionary, &mut hasher);
    FontKey((1 << 63) | (hasher.finish() >> 1))
}

fn hash_dictionary(dictionary: &Dictionary, hasher: &mut DefaultHasher) {
    dictionary.len().hash(hasher);
    for (key, value) in dictionary {
        key.hash(hasher);
        hash_value(value, hasher);
    }
}

fn hash_value(value: &Value, hasher: &mut DefaultHasher) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        Value::Boolean(b) => b.hash(hasher),
        Value::Integer(n) => n.hash(hasher),
        Value::Real(r) => r.to_bits().hash(hasher),
        Value::String(bytes) => bytes.hash(hasher),
        Value::Name(name) => name.hash(hasher),
        Value::Array(values) => {
            values.len().hash(hasher);
            for value in values {
                hash_value(value, hasher);
            }
        }
        Value::Dictionary(dictionary) => hash_dictionary(dictionary, hasher),
        Value::Reference(id) => id.hash(hasher),
        Value::Null => {}
    }
}

/// `FontBBox` of the font descriptor (of the descendant of a Type 0 font,
/// of the font itself for Type 3), when it can be read.
fn font_box(document: &Document, dictionary: &Dictionary, kind: FontKind) -> Option<Rect> {
    let dictionary = if kind == FontKind::Type0 {
        let first = document
            .resolve(dictionary.get(b"DescendantFonts")?)
            .ok()?
            .as_array()?
            .first()?;
        document.resolve_dictionary(first).ok()?
    } else {
        dictionary
    };
    let dictionary = if kind == FontKind::Type3 {
        dictionary
    } else {
        document
            .resolve_dictionary(dictionary.get(b"FontDescriptor")?)
            .ok()?
    };
    document
        .resolve(dictionary.get(b"FontBBox")?)
        .ok()?
        .as_array()
        .and_then(Rect::from_array)
}
