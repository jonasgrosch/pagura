//! The page tree (ISO 32000-2 clause 7.7.3) as typed pages.

use crate::{Document, tags};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Name, ObjectId, Rect, Result, Value,
};
use std::collections::BTreeSet;

/// Page attributes a page inherits from its ancestors (Table 31).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// A branch missing its required Type is identifiable by reference-only Kids
/// (7.7.3.2). The traversal reports the inference before following any children.
pub(crate) fn branch(document: &Document, dictionary: &Dictionary) -> Result<bool> {
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

/// Clockwise page rotation for display (`/Rotate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// Upright.
    #[default]
    None,
    /// 90° clockwise.
    Clockwise90,
    /// 180°.
    Clockwise180,
    /// 270° clockwise.
    Clockwise270,
}

impl Rotation {
    /// Interprets a `/Rotate` value, which must be a multiple of 90 and
    /// may be negative or exceed 360.
    pub fn from_degrees(degrees: i64) -> Option<Self> {
        match degrees.rem_euclid(360) {
            0 => Some(Self::None),
            90 => Some(Self::Clockwise90),
            180 => Some(Self::Clockwise180),
            270 => Some(Self::Clockwise270),
            _ => None,
        }
    }

    /// The rotation in degrees, 0–270.
    pub fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 90,
            Self::Clockwise180 => 180,
            Self::Clockwise270 => 270,
        }
    }
}

/// A page with inherited attributes applied and boxes validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    id: ObjectId,
    index: usize,
    media_box: Rect,
    crop_box: Rect,
    rotation: Rotation,
    user_unit: f64,
    resources: Dictionary,
    dictionary: Dictionary,
}

impl Page {
    /// The page object's identity.
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

    /// The resource dictionary, resolved; empty if the page has none.
    pub fn resources(&self) -> &Dictionary {
        &self.resources
    }

    /// The page dictionary with inherited attributes filled in.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }
}

/// Walks the page tree from the catalog's `/Pages` root.
pub(crate) fn read(document: &Document) -> Result<Vec<Page>> {
    let root = document
        .catalog()?
        .get(b"Pages")
        .and_then(Value::as_reference)
        .ok_or(Error::syntax("catalog Pages must be an indirect reference"))?;
    let mut walk = Walk {
        document,
        visited: BTreeSet::new(),
        pages: Vec::new(),
        diagnostics: Diagnostics::new(document.recovery()),
    };
    walk.node(root, &Dictionary::new(), 0)?;
    document.record(walk.diagnostics.take());
    Ok(walk.pages)
}

struct Walk<'a> {
    document: &'a Document,
    visited: BTreeSet<ObjectId>,
    pages: Vec<Page>,
    diagnostics: Diagnostics,
}

impl Walk<'_> {
    fn node(&mut self, id: ObjectId, inherited: &Dictionary, depth: usize) -> Result<()> {
        let limits = self.document.limits();
        if depth > limits.depth {
            return Err(Error::limit("page tree depth budget exceeded").in_object(id));
        }
        if !self.visited.insert(id) {
            return Err(Error::cycle("page tree node is reachable twice").in_object(id));
        }
        let mut dictionary = self
            .document
            .object(id)?
            .dictionary()
            .ok_or_else(|| Error::syntax("page tree node must be a dictionary").in_object(id))?
            .clone();
        // Leaf evidence comes from the node itself: an inherited MediaBox
        // would make every untyped, childless node look like a page.
        let leaf = !dictionary.contains_key(b"Kids")
            && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"));
        for key in INHERITABLE {
            if !dictionary.contains_key(key)
                && let Some(value) = inherited.get(key)
            {
                dictionary.insert(Name::new(key), value.clone());
            }
        }
        if !dictionary.contains_key(b"Type") {
            let inferred = if branch(self.document, &dictionary)? {
                b"Pages".as_slice()
            } else if leaf {
                b"Page"
            } else {
                return Err(Error::syntax("page tree node has no identifiable Type").in_object(id));
            };
            self.diagnostics.repair(
                Diagnostic::new(
                    tags::PAGE_TYPE,
                    "missing page-tree Type inferred from Kids or leaf attributes",
                )
                .at(match self.document.object(id)?.source() {
                    crate::ObjectSource::File(range) => pagura_core::Position::File(range.start),
                    crate::ObjectSource::Compressed { .. } => pagura_core::Position::Unknown,
                })
                .in_object(id),
            )?;
            dictionary.insert(Name::new(b"Type"), Value::name(inferred));
        }
        if dictionary.is_name(b"Type", b"Pages") {
            self.intermediate(id, &dictionary, depth)
        } else if dictionary.is_name(b"Type", b"Page") {
            if self.pages.len() >= limits.pages {
                return Err(Error::limit("page count budget exceeded").in_object(id));
            }
            let page = self.leaf(id, dictionary)?;
            self.pages.push(page);
            Ok(())
        } else {
            Err(Error::syntax("page tree node Type must be Pages or Page").in_object(id))
        }
    }

    fn intermediate(&mut self, id: ObjectId, dictionary: &Dictionary, depth: usize) -> Result<()> {
        let kids = dictionary
            .get(b"Kids")
            .map(|kids| self.document.resolve(kids))
            .transpose()?
            .and_then(Value::as_array)
            .ok_or_else(|| Error::syntax("Pages node must have a Kids array").in_object(id))?;
        let before = self.pages.len();
        for kid in kids {
            let kid = kid
                .as_reference()
                .ok_or_else(|| Error::syntax("Kids must hold indirect references").in_object(id))?;
            self.node(kid, dictionary, depth + 1)?;
        }
        let found = i64::try_from(self.pages.len() - before).unwrap_or(i64::MAX);
        if dictionary.get(b"Count").and_then(Value::as_integer) != Some(found) {
            self.diagnostics.repair(
                Diagnostic::new(
                    tags::PAGE_COUNT,
                    "Pages Count disagrees with its leaves; leaves used",
                )
                .in_object(id),
            )?;
        }
        Ok(())
    }

    fn leaf(&self, id: ObjectId, dictionary: Dictionary) -> Result<Page> {
        let document = self.document;
        let rect = |key: &[u8]| -> Result<Option<Rect>> {
            let Some(value) = dictionary.get(key) else {
                return Ok(None);
            };
            let values = document.resolve(value)?.as_array().unwrap_or_default();
            let elements = values
                .iter()
                .map(|value| document.resolve(value).cloned())
                .collect::<Result<Vec<_>>>()?;
            Rect::from_array(&elements)
                .filter(|rect| !rect.is_empty())
                .map(Some)
                .ok_or_else(|| {
                    Error::syntax("page box must be four numbers enclosing an area").in_object(id)
                })
        };
        let media_box = rect(b"MediaBox")?
            .ok_or_else(|| Error::syntax("page has no MediaBox").in_object(id))?;
        let crop_box = match rect(b"CropBox")? {
            None => media_box,
            Some(crop) => crop
                .intersection(&media_box)
                .ok_or_else(|| Error::syntax("CropBox does not overlap MediaBox").in_object(id))?,
        };
        let rotation = match dictionary.get(b"Rotate") {
            None => Rotation::None,
            Some(value) => document
                .resolve(value)?
                .as_integer()
                .and_then(Rotation::from_degrees)
                .ok_or_else(|| Error::syntax("Rotate must be a multiple of 90").in_object(id))?,
        };
        let user_unit = match dictionary.get(b"UserUnit") {
            None => 1.0,
            Some(value) => document
                .resolve(value)?
                .as_number()
                .filter(|&unit| unit > 0.0)
                .ok_or_else(|| Error::syntax("UserUnit must be a positive number").in_object(id))?,
        };
        let resources = match dictionary.get(b"Resources") {
            None => Dictionary::new(),
            Some(value) => match document.resolve(value)? {
                Value::Dictionary(resources) => resources.clone(),
                Value::Null => Dictionary::new(),
                _ => return Err(Error::syntax("Resources must be a dictionary").in_object(id)),
            },
        };
        Ok(Page {
            id,
            index: self.pages.len(),
            media_box,
            crop_box,
            rotation,
            user_unit,
            resources,
            dictionary,
        })
    }
}
