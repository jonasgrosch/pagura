//! Appearance streams kept as data.

use pagura_core::{Dictionary, Error, Name, ObjectId, Rect, Result, Value};
use pagura_document::Document;

/// The normal appearance selected for one widget (12.5.5). The set is
/// closed: an appearance is a stored stream, nothing, or generated data.
#[derive(Debug, Clone, PartialEq)]
pub enum Appearance {
    /// An existing indirect stream, selected using the replacement AS state.
    Existing(ObjectId),
    /// An undefined appearance state paints nothing (12.5.5).
    Hidden,
    /// An unencoded appearance program and its complete form dictionary.
    Generated(AppearanceStream),
}

/// An appearance stream kept as data, without allocating an indirect object
/// ID: a complete Form XObject dictionary and its decoded content (8.10).
///
/// The dictionary carries `BBox`, `Resources` and, when the appearance
/// replaces the text of an existing stream, that stream's other entries
/// (such as `Matrix` and `Group`), which may still refer to objects of the
/// source document. Read them as a stored form's entries are read.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceStream {
    dictionary: Dictionary,
    content: Vec<u8>,
}

impl AppearanceStream {
    /// Complete Form XObject dictionary, including BBox and Resources.
    /// Filter, DecodeParms and Length are absent: content is decoded data.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Content program, before any stream encoding.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Estimated caller-owned storage; source references count only their IDs.
    pub fn retained_bytes(&self) -> usize {
        self.dictionary
            .retained_bytes()
            .saturating_add(self.content.capacity())
            .saturating_add(size_of::<Self>())
    }

    /// Keeps `dictionary` and `content` as a stream within the document's
    /// storage budget.
    pub(crate) fn new(
        document: &Document,
        dictionary: Dictionary,
        content: Vec<u8>,
    ) -> Result<Self> {
        let stream = Self {
            dictionary,
            content,
        };
        if stream.retained_bytes() > document.limits().document_bytes {
            return Err(Error::limit("generated appearance storage budget exceeded"));
        }
        Ok(stream)
    }
}

/// A Form XObject dictionary with `bbox` and the identity matrix (Table 93).
pub(crate) fn form_dictionary(bbox: Rect) -> Dictionary {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type"), Value::name(b"XObject"));
    dictionary.insert(Name::new(b"Subtype"), Value::name(b"Form"));
    dictionary.insert(Name::new(b"FormType"), Value::Integer(1));
    dictionary.insert(
        Name::new(b"BBox"),
        Value::Array(
            [bbox.x0(), bbox.y0(), bbox.x1(), bbox.y1()]
                .map(Value::Real)
                .to_vec(),
        ),
    );
    dictionary
}
