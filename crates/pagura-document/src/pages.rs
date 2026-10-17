//! The page tree (ISO 32000-2 clause 7.7.3) as typed pages.

use crate::{Document, tags};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, ObjectId, Rect, Result, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
    resources: Arc<Dictionary>,
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

    /// The resource dictionary, resolved and possibly inherited; empty if the
    /// page has none. Pages sharing a resource dictionary share one copy.
    pub fn resources(&self) -> &Dictionary {
        &self.resources
    }

    /// The page object's own dictionary. Inherited attributes (Table 31) are
    /// not copied in; the typed accessors above resolve them.
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
        shared: BTreeMap::new(),
        diagnostics: Diagnostics::new(document.recovery()),
    };
    walk.node(root, Inherited::default(), 0)?;
    document.record(walk.diagnostics.take());
    Ok(walk.pages)
}

/// Attributes a page inherits from the nearest ancestor defining them
/// (Table 31), borrowed from the document's object cache.
#[derive(Clone, Copy, Default)]
struct Inherited<'a> {
    /// The defining node and its Resources entry, resolved lazily per page.
    resources: Option<(ObjectId, &'a Value)>,
    media_box: Option<&'a Value>,
    crop_box: Option<&'a Value>,
    rotate: Option<&'a Value>,
}

/// Identity of one resource dictionary: an indirect object, or the direct
/// Resources entry of the page-tree node that defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResourceKey {
    Object(ObjectId),
    DirectIn(ObjectId),
}

struct Walk<'a> {
    document: &'a Document,
    visited: BTreeSet<ObjectId>,
    pages: Vec<Page>,
    /// One resolved copy per resource dictionary, shared by every page using it.
    shared: BTreeMap<ResourceKey, Arc<Dictionary>>,
    diagnostics: Diagnostics,
}

impl<'a> Walk<'a> {
    fn node(&mut self, id: ObjectId, inherited: Inherited<'a>, depth: usize) -> Result<()> {
        let limits = self.document.limits();
        if depth > limits.depth {
            return Err(Error::limit("page tree depth budget exceeded").in_object(id));
        }
        if !self.visited.insert(id) {
            return Err(Error::cycle("page tree node is reachable twice").in_object(id));
        }
        let object = self.document.object(id)?;
        let dictionary = object
            .dictionary()
            .ok_or_else(|| Error::syntax("page tree node must be a dictionary").in_object(id))?;
        let inherited = Inherited {
            resources: dictionary
                .get(b"Resources")
                .map(|value| (id, value))
                .or(inherited.resources),
            media_box: dictionary.get(b"MediaBox").or(inherited.media_box),
            crop_box: dictionary.get(b"CropBox").or(inherited.crop_box),
            rotate: dictionary.get(b"Rotate").or(inherited.rotate),
        };
        let branch = if dictionary.is_name(b"Type", b"Pages") {
            true
        } else if dictionary.is_name(b"Type", b"Page") {
            false
        } else if dictionary.contains_key(b"Type") {
            return Err(Error::syntax("page tree node Type must be Pages or Page").in_object(id));
        } else {
            // Leaf evidence comes from the node itself: an inherited MediaBox
            // would make every untyped, childless node look like a page.
            let branch = branch(self.document, dictionary)?;
            let leaf = !dictionary.contains_key(b"Kids")
                && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"));
            if !branch && !leaf {
                return Err(Error::syntax("page tree node has no identifiable Type").in_object(id));
            }
            self.diagnostics.repair(
                Diagnostic::new(
                    tags::PAGE_TYPE,
                    "missing page-tree Type inferred from Kids or leaf attributes",
                )
                .at(match object.source() {
                    crate::ObjectSource::File(range) => pagura_core::Position::File(range.start),
                    crate::ObjectSource::Compressed { .. } => pagura_core::Position::Unknown,
                })
                .in_object(id),
            )?;
            branch
        };
        if branch {
            self.intermediate(id, dictionary, inherited, depth)
        } else {
            if self.pages.len() >= limits.pages {
                return Err(Error::limit("page count budget exceeded").in_object(id));
            }
            let page = self.leaf(id, dictionary, inherited)?;
            self.pages.push(page);
            Ok(())
        }
    }

    fn intermediate(
        &mut self,
        id: ObjectId,
        dictionary: &'a Dictionary,
        inherited: Inherited<'a>,
        depth: usize,
    ) -> Result<()> {
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
            self.node(kid, inherited, depth + 1)?;
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

    /// Resolves the inherited Resources entry once per resource dictionary.
    fn resources(&mut self, page: ObjectId, inherited: Inherited<'a>) -> Result<Arc<Dictionary>> {
        let Some((node, value)) = inherited.resources else {
            return Ok(Arc::default());
        };
        let key = match value {
            Value::Reference(reference) => ResourceKey::Object(*reference),
            _ => ResourceKey::DirectIn(node),
        };
        if let Some(shared) = self.shared.get(&key) {
            return Ok(Arc::clone(shared));
        }
        let resources = match self.document.resolve(value)? {
            Value::Dictionary(resources) => Arc::new(resources.clone()),
            Value::Null => Arc::default(),
            _ => return Err(Error::syntax("Resources must be a dictionary").in_object(page)),
        };
        self.shared.insert(key, Arc::clone(&resources));
        Ok(resources)
    }

    fn leaf(
        &mut self,
        id: ObjectId,
        dictionary: &Dictionary,
        inherited: Inherited<'a>,
    ) -> Result<Page> {
        let document = self.document;
        let rect = |value: Option<&Value>| -> Result<Option<Rect>> {
            let Some(value) = value else {
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
        let media_box = rect(inherited.media_box)?
            .ok_or_else(|| Error::syntax("page has no MediaBox").in_object(id))?;
        let crop_box = match rect(inherited.crop_box)? {
            None => media_box,
            Some(crop) => crop
                .intersection(&media_box)
                .ok_or_else(|| Error::syntax("CropBox does not overlap MediaBox").in_object(id))?,
        };
        let rotation = match inherited.rotate {
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
        let resources = self.resources(id, inherited)?;
        Ok(Page {
            id,
            index: self.pages.len(),
            media_box,
            crop_box,
            rotation,
            user_unit,
            resources,
            dictionary: dictionary.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, OpenOptions};
    use pagura_testkit::fixture;
    use std::sync::Arc;

    #[test]
    fn pages_share_one_copy_of_each_resource_dictionary() {
        let document = Document::open(
            fixture("document", "pages-shared-resources.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let pages = document.pages().unwrap();
        let [inherited, sibling, direct, own] = pages else {
            panic!("expected four pages, found {}", pages.len());
        };
        // Pages 3, 4 and 9 use object 7 (two inherit it, one names it itself).
        assert!(Arc::ptr_eq(&inherited.resources, &sibling.resources));
        assert!(Arc::ptr_eq(&inherited.resources, &own.resources));
        assert!(inherited.resources().contains_key(b"ExtGState"));
        // Page 8 inherits node 5's direct dictionary instead.
        assert!(!Arc::ptr_eq(&inherited.resources, &direct.resources));
        assert!(!direct.resources().contains_key(b"ExtGState"));
        assert_eq!(direct.media_box(), inherited.media_box());
        // The page dictionary is the page object's own, without inherited entries.
        assert!(!inherited.dictionary().contains_key(b"Resources"));
        assert!(!inherited.dictionary().contains_key(b"MediaBox"));
    }
}
