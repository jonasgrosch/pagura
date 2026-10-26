//! The page tree (ISO 32000-2 clause 7.7.3) as typed pages.

pub(crate) mod contents;
mod directory;
mod loading;

pub use loading::{PageJob, PageLoading};
pub(crate) mod geometry;

pub(crate) use directory::read;

use crate::Document;
use crate::source::ReadResult;
use pagura_core::syntax::PathComponent;
use pagura_core::{Dictionary, ObjectId, Rect, Rotation, Value};
use std::sync::{Arc, Weak};

/// A branch missing its required Type is identifiable by reference-only Kids
/// (7.7.3.2). The traversal reports the inference before following any children.
pub(crate) fn branch(document: &Document, dictionary: &Dictionary) -> ReadResult<bool> {
    if dictionary.is_name(b"Type", b"Pages") {
        return Ok(true);
    }
    if dictionary.contains_key(b"Type") {
        return Ok(false);
    }
    let Some(kids) = dictionary.get(b"Kids") else {
        return Ok(false);
    };
    Ok(document
        .resolve(kids)?
        .as_array()
        .is_some_and(|kids| kids.iter().all(|kid| matches!(kid, Value::Reference(_)))))
}

/// A page with inherited attributes applied and boxes validated.
#[derive(Debug, Clone)]
pub struct Page {
    document: Weak<()>,
    id: ObjectId,
    index: usize,
    media_box: Rect,
    crop_box: Rect,
    rotation: Rotation,
    user_unit: f64,
    resources: Arc<Dictionary>,
    dictionary: Dictionary,
    source_path: Vec<PathComponent>,
    /// Owned selected-page results retain their charge across caller clones.
    _reservation: Option<Arc<crate::memory::Reservation>>,
}

impl Page {
    /// The page object's identity. For a repaired direct Kids dictionary this
    /// is the containing indirect object's identity; [`Self::index`] distinguishes
    /// its page occurrences. The original dictionary is available unchanged.
    pub fn id(&self) -> ObjectId {
        self.id
    }

    /// Zero-based position in document order.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The media box in default user space.
    pub fn media_box(&self) -> Rect {
        self.media_box
    }

    /// The crop box, already intersected with the media box.
    pub fn crop_box(&self) -> Rect {
        self.crop_box
    }

    /// Display rotation.
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    /// Size of a default user-space unit in 1/72 inch (`/UserUnit`).
    pub fn user_unit(&self) -> f64 {
        self.user_unit
    }

    /// The resource dictionary, resolved and possibly inherited; empty if the
    /// page has none. Pages sharing a resource dictionary share one copy.
    pub fn resources(&self) -> &Dictionary {
        &self.resources
    }

    pub(crate) fn source_path(&self) -> &[PathComponent] {
        &self.source_path
    }

    /// The token of the document this page came from.
    pub(crate) const fn document(&self) -> &Weak<()> {
        &self.document
    }

    /// The page object's own dictionary. Inherited attributes (Table 31) are
    /// not copied in; the typed accessors above resolve them.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }
}

impl PartialEq for Page {
    /// The same page of the same document.
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.document, &other.document)
            && self.id == other.id
            && self.index == other.index
            && self.media_box == other.media_box
            && self.crop_box == other.crop_box
            && self.rotation == other.rotation
            && self.user_unit == other.user_unit
            && self.resources == other.resources
            && self.dictionary == other.dictionary
            && self.source_path == other.source_path
    }
}

#[cfg(test)]
mod tests;
