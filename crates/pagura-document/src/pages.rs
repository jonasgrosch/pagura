//! The page tree (ISO 32000-2 clause 7.7.3) as typed pages.

pub(crate) mod contents;
pub(crate) mod geometry;

use crate::{Document, repairs};
use pagura_core::syntax::PathComponent;
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
    source_path: Vec<PathComponent>,
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
        active: BTreeSet::new(),
        parents: None,
        omitted: false,
        steps: 0,
        pages: Vec::new(),
        shared: BTreeMap::new(),
        diagnostics: Diagnostics::new(document.recovery()),
        reservation: document.retained.reservation(),
    };
    walk.node(root, Inherited::default(), None, 0)?;
    if walk.pages.is_empty() && walk.omitted {
        let cyclic = walk
            .diagnostics
            .as_slice()
            .iter()
            .any(|item| item.repair() == Some(repairs::PAGE_CYCLE));
        return Err(if cyclic {
            Error::cycle("cyclic page tree contains no usable leaves")
        } else {
            Error::syntax("damaged page tree contains no usable leaves")
        }
        .in_object(root));
    }
    document.report(walk.diagnostics.take());
    walk.reservation.commit();
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
    Empty,
    Object(ObjectId),
    DirectIn(usize),
}

struct Walk<'a> {
    document: &'a Document,
    visited: BTreeSet<ObjectId>,
    active: BTreeSet<ObjectId>,
    parents: Option<BTreeMap<ObjectId, Vec<ObjectId>>>,
    omitted: bool,
    steps: usize,
    pages: Vec<Page>,
    /// One resolved copy per resource dictionary, shared by every page using it.
    shared: BTreeMap<ResourceKey, Arc<Dictionary>>,
    diagnostics: Diagnostics,
    reservation: crate::memory::Reservation<'a>,
}

impl<'a> Walk<'a> {
    fn node(
        &mut self,
        id: ObjectId,
        inherited: Inherited<'a>,
        parent: Option<ObjectId>,
        depth: usize,
    ) -> Result<()> {
        let limits = self.document.limits();
        if depth > limits.depth {
            return Err(Error::limit("page tree depth budget exceeded").in_object(id));
        }
        if self.active.contains(&id) {
            self.diagnostics.repair(
                Diagnostic::new(repairs::PAGE_CYCLE, "cyclic page-tree child edge omitted")
                    .in_object(id),
            )?;
            self.omitted = true;
            return Ok(());
        }
        let repeated = !self.visited.insert(id);
        let object = self.document.object(id)?;
        let dictionary = object
            .dictionary()
            .ok_or_else(|| Error::syntax("page tree node must be a dictionary").in_object(id))?;
        if repeated {
            self.diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_REPEAT,
                    "repeated page-tree node retained in Kids order",
                )
                .in_object(id),
            )?;
        }
        self.active.insert(id);
        let result = self.dictionary_node(id, dictionary, inherited, parent, depth, &[]);
        self.active.remove(&id);
        result
    }

    fn dictionary_node(
        &mut self,
        id: ObjectId,
        dictionary: &'a Dictionary,
        inherited: Inherited<'a>,
        parent: Option<ObjectId>,
        depth: usize,
        source_path: &[PathComponent],
    ) -> Result<()> {
        let limits = self.document.limits();
        if self.steps >= limits.value_nodes {
            return Err(Error::limit("page-tree traversal work budget exceeded").in_object(id));
        }
        self.steps += 1;
        if depth > limits.depth {
            return Err(Error::limit("page tree depth budget exceeded").in_object(id));
        }
        if dictionary
            .get(b"Parent")
            .is_some_and(|value| value.as_reference() != parent)
        {
            self.diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_PARENT,
                    "Parent disagrees with traversed Kids edge; Kids ancestry used",
                )
                .in_object(id),
            )?;
        }
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
        } else if dictionary.is_name(b"Type", b"Template")
            && parent.is_some()
            && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"))
        {
            self.diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_TYPE_INVALID,
                    "Template in Kids displayed as a Page",
                )
                .in_object(id),
            )?;
            false
        } else if dictionary.contains_key(b"Type") {
            return Err(Error::syntax("page tree node Type must be Pages or Page").in_object(id));
        } else {
            // Leaf evidence comes from the node itself: an inherited MediaBox
            // would make every untyped, childless node look like a page.
            let leaf = !dictionary.contains_key(b"Kids")
                && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"));
            let branch = branch(self.document, dictionary)?
                || (!leaf && !dictionary.contains_key(b"Kids") && !self.backlinks(id)?.is_empty());
            if !branch && !leaf {
                return Err(Error::syntax("page tree node has no identifiable Type").in_object(id));
            }
            self.diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_TYPE,
                    "missing page-tree Type inferred from Kids or leaf attributes",
                )
                .at(match self.document.object(id)?.source() {
                    crate::ObjectSource::File(range) => pagura_core::Position::File(range.start),
                    crate::ObjectSource::Compressed { .. } => pagura_core::Position::Unknown,
                })
                .in_object(id),
            )?;
            branch
        };
        if branch {
            self.intermediate(id, dictionary, inherited, depth, source_path)
        } else {
            if self.pages.len() >= limits.pages {
                return Err(Error::limit("page count budget exceeded").in_object(id));
            }
            let page = self.leaf(id, dictionary, inherited, source_path)?;
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
        source_path: &[PathComponent],
    ) -> Result<()> {
        let indirect = dictionary
            .get(b"Kids")
            .is_some_and(|value| matches!(value, Value::Reference(_)));
        let (kids, owner) = self.kids_value(id, dictionary)?;
        let before = self.pages.len();
        match kids {
            Some(Value::Array(kids)) => {
                for (index, kid) in kids.iter().enumerate() {
                    if let Some(kid) = kid.as_reference()
                        && self.document.slots.contains_key(&kid)
                    {
                        self.node(kid, inherited, Some(id), depth + 1)?;
                        continue;
                    }
                    if let Value::Dictionary(dictionary) = kid {
                        self.diagnostics.repair(
                            Diagnostic::new(
                                repairs::PAGE_DIRECT,
                                "direct Kids dictionary used with its containing object's identity",
                            )
                            .in_object(id),
                        )?;
                        let mut path = if indirect {
                            Vec::new()
                        } else {
                            source_path.to_vec()
                        };
                        if !indirect {
                            path.push(PathComponent::Key(pagura_core::Name::new(b"Kids")));
                        }
                        path.push(PathComponent::Index(index));
                        self.dictionary_node(
                            owner,
                            dictionary,
                            inherited,
                            Some(id),
                            depth + 1,
                            &path,
                        )?;
                        continue;
                    }
                    self.diagnostics.repair(
                        Diagnostic::new(
                            repairs::PAGE_KIDS,
                            "invalid or undefined Kids entry omitted",
                        )
                        .in_object(id),
                    )?;
                    self.omitted = true;
                }
            }
            _ if dictionary
                .get(b"Kids")
                .and_then(Value::as_reference)
                .is_some()
                && kids.is_some_and(|value| value.as_dictionary().is_some()) =>
            {
                self.diagnostics.repair(
                    Diagnostic::new(
                        repairs::PAGE_KIDS,
                        "single Kids reference treated as a one-element array",
                    )
                    .in_object(id),
                )?;
                self.node(owner, inherited, Some(id), depth + 1)?;
            }
            _ => self.backlinked_kids(id, inherited, depth, source_path)?,
        }
        let found = i64::try_from(self.pages.len() - before).unwrap_or(i64::MAX);
        if dictionary.get(b"Count").and_then(Value::as_integer) != Some(found) {
            self.diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_COUNT,
                    "Pages Count disagrees with its leaves; leaves used",
                )
                .in_object(id),
            )?;
        }
        Ok(())
    }

    /// Walks the explicit Parent backlinks of a Pages node without usable Kids.
    fn backlinked_kids(
        &mut self,
        id: ObjectId,
        inherited: Inherited<'a>,
        depth: usize,
        source_path: &[PathComponent],
    ) -> Result<()> {
        if !source_path.is_empty() {
            return Err(Error::syntax("direct Pages dictionary has no usable Kids").in_object(id));
        }
        let kids = self.backlinks(id)?.to_vec();
        if kids.is_empty() {
            return Err(
                Error::syntax("Pages node has no Kids or explicit Parent backlinks").in_object(id),
            );
        }
        self.diagnostics.repair(
            Diagnostic::new(
                repairs::PAGE_KIDS_REBUILT,
                "Kids recovered from explicit Parent backlinks in object-number order",
            )
            .in_object(id),
        )?;
        for kid in kids {
            self.node(kid, inherited, Some(id), depth + 1)?;
        }
        Ok(())
    }

    fn kids_value(
        &self,
        id: ObjectId,
        dictionary: &'a Dictionary,
    ) -> Result<(Option<&'a Value>, ObjectId)> {
        let mut kids = dictionary.get(b"Kids");
        let mut owner = id;
        for _ in 0..=self.document.limits().depth {
            let Some(Value::Reference(reference)) = kids else {
                break;
            };
            owner = *reference;
            if !self.document.slots.contains_key(reference) {
                kids = None;
                break;
            }
            kids = Some(self.document.object(*reference)?.value());
        }
        if kids.is_some_and(|value| matches!(value, Value::Reference(_))) {
            return Err(Error::limit("Kids reference chain too long").in_object(id));
        }
        Ok((kids, owner))
    }

    /// Built at most once. Only explicit typed Page/Pages children contribute;
    /// inherited attributes and physical EOF never establish a node's identity.
    fn backlinks(&mut self, parent: ObjectId) -> Result<&[ObjectId]> {
        if self.parents.is_none() {
            let mut parents: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
            for id in self.document.object_ids() {
                let object = match self.document.object(id) {
                    Ok(object) => object,
                    Err(error) if crate::xref::structural(&error) => continue,
                    Err(error) => return Err(error),
                };
                if let Some(dictionary) = object.dictionary()
                    && (dictionary.is_name(b"Type", b"Page")
                        || dictionary.is_name(b"Type", b"Pages"))
                    && let Some(parent) = dictionary.get(b"Parent").and_then(Value::as_reference)
                {
                    parents.entry(parent).or_default().push(id);
                }
            }
            self.parents = Some(parents);
        }
        Ok(self
            .parents
            .as_ref()
            .and_then(|parents| parents.get(&parent))
            .map_or(&[], Vec::as_slice))
    }

    /// Resolves the inherited Resources entry once per resource dictionary.
    fn resources(&mut self, page: ObjectId, inherited: Inherited<'a>) -> Result<Arc<Dictionary>> {
        let key = match inherited.resources {
            None => ResourceKey::Empty,
            Some((_, Value::Reference(reference))) => ResourceKey::Object(*reference),
            Some((_, value)) => ResourceKey::DirectIn(std::ptr::from_ref(value) as usize),
        };
        if let Some(shared) = self.shared.get(&key) {
            return Ok(Arc::clone(shared));
        }
        let value = inherited
            .resources
            .map(|(_, value)| self.document.resolve(value))
            .transpose()?;
        let dictionary = match value {
            Some(Value::Dictionary(resources)) => Some(resources),
            None | Some(Value::Null) => None,
            _ => return Err(Error::syntax("Resources must be a dictionary").in_object(page)),
        };
        let resources =
            dictionary.map_or_else(Arc::default, |dictionary| Arc::new(dictionary.clone()));
        let bytes = resources
            .retained_bytes()
            .saturating_add(2 * size_of::<usize>());
        self.reservation.charge(bytes, page)?;
        self.shared.insert(key, Arc::clone(&resources));
        Ok(resources)
    }

    fn leaf(
        &mut self,
        id: ObjectId,
        dictionary: &Dictionary,
        inherited: Inherited<'a>,
        source_path: &[PathComponent],
    ) -> Result<Page> {
        let document = self.document;
        let (media_box, crop_box) = geometry::boxes(
            document,
            id,
            inherited.media_box,
            inherited.crop_box,
            &mut self.diagnostics,
        )?;
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
        let dictionary = dictionary.clone();
        let bytes = size_of::<Page>().saturating_add(
            dictionary
                .retained_bytes()
                .saturating_sub(size_of::<Dictionary>()),
        );
        let source_path = source_path.to_vec();
        let bytes = source_path.iter().fold(
            bytes.saturating_add(
                source_path
                    .capacity()
                    .saturating_mul(size_of::<PathComponent>()),
            ),
            |sum, part| {
                sum.saturating_add(
                    part.retained_bytes()
                        .saturating_sub(size_of::<PathComponent>()),
                )
            },
        );

        self.reservation.charge(bytes, id)?;
        Ok(Page {
            id,
            index: self.pages.len(),
            media_box,
            crop_box,
            rotation,
            user_unit,
            resources,
            dictionary,
            source_path,
        })
    }
}

#[cfg(test)]
mod tests;
