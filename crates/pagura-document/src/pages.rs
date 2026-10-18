//! The page tree (ISO 32000-2 clause 7.7.3) as typed pages.

use crate::{Document, tags};
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
            .any(|item| item.repair() == Some(tags::PAGE_CYCLE));
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
                Diagnostic::new(tags::PAGE_CYCLE, "cyclic page-tree child edge omitted")
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
                    tags::PAGE_REPEAT,
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
                    tags::PAGE_PARENT,
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
                    tags::PAGE_TYPE_INVALID,
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
                    tags::PAGE_TYPE,
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
                                tags::PAGE_DIRECT,
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
                        Diagnostic::new(tags::PAGE_KIDS, "invalid or undefined Kids entry omitted")
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
                        tags::PAGE_KIDS,
                        "single Kids reference treated as a one-element array",
                    )
                    .in_object(id),
                )?;
                self.node(owner, inherited, Some(id), depth + 1)?;
            }
            _ => {
                if !source_path.is_empty() {
                    return Err(
                        Error::syntax("direct Pages dictionary has no usable Kids").in_object(id)
                    );
                }
                let kids = self.backlinks(id)?.to_vec();
                if kids.is_empty() {
                    return Err(Error::syntax(
                        "Pages node has no Kids or explicit Parent backlinks",
                    )
                    .in_object(id));
                }
                self.diagnostics.repair(
                    Diagnostic::new(
                        tags::PAGE_KIDS_REBUILT,
                        "Kids recovered from explicit Parent backlinks in object-number order",
                    )
                    .in_object(id),
                )?;
                for kid in kids {
                    self.node(kid, inherited, Some(id), depth + 1)?;
                }
            }
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
        let (media_box, crop_box) = crate::page_geometry::boxes(
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

#[cfg(test)]
mod retained_tests {
    use super::Page;
    use crate::{Document, OpenOptions};
    use pagura_core::{Dictionary, ErrorKind, Limits};
    use pagura_testkit::fixture;
    use std::sync::Arc;

    fn open(bytes: usize) -> Document {
        let document = Document::open(
            fixture("document", "pages-shared-resources.pdf"),
            &OpenOptions {
                limits: Limits {
                    document_bytes: bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        for id in document.object_ids() {
            document.object(id).unwrap();
        }
        document
    }

    #[test]
    fn shared_resource_storage_is_charged_once_and_failed_walks_release_reservations() {
        let document = open(usize::MAX);
        let initial = document.retained.used();
        let pages = document.pages().unwrap();
        let mut unique = Vec::new();
        let mut expected = 0;
        for page in pages {
            expected +=
                size_of::<Page>() + page.dictionary.retained_bytes() - size_of::<Dictionary>();
            if !unique
                .iter()
                .any(|resource| Arc::ptr_eq(resource, &page.resources))
            {
                unique.push(Arc::clone(&page.resources));
                expected += page.resources.retained_bytes() + 2 * size_of::<usize>();
            }
        }
        assert_eq!(unique.len(), 2);
        assert_eq!(document.retained.used(), initial + expected);
        let exact = open(initial + expected);
        assert_eq!(exact.page_count().unwrap(), 4);
        assert_eq!(exact.retained.used(), initial + expected);
        exact.pages().unwrap();
        assert_eq!(exact.retained.used(), initial + expected);
        let limited = open(initial + expected - 1);
        let error = limited.pages().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(error.object.is_some());
        assert_eq!(limited.retained.used(), initial);
        assert_eq!(limited.pages().unwrap_err(), error);
        assert_eq!(limited.retained.used(), initial);
    }
}

#[cfg(test)]
mod recovery_tests {
    use crate::{Document, OpenOptions, tags};
    use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Rect};
    use pagura_testkit::fixture;
    use std::sync::Arc;

    #[test]
    fn page_repairs_reject_strictly_and_preserve_source_and_geometry() {
        for (name, tag, count) in [
            ("pages-media-missing", tags::PAGE_MEDIA_BOX, 1),
            ("pages-media-empty", tags::PAGE_MEDIA_BOX, 1),
            ("pages-media-from-crop", tags::PAGE_MEDIA_BOX, 1),
            ("pages-crop-empty", tags::PAGE_CROP_BOX, 1),
            ("pages-crop-disjoint", tags::PAGE_CROP_BOX, 1),
            ("pages-kids-invalid", tags::PAGE_KIDS, 1),
            ("pages-kids-single", tags::PAGE_KIDS, 1),
            ("pages-kids-rebuilt", tags::PAGE_KIDS_REBUILT, 1),
            ("pages-root-untyped-backlinks", tags::PAGE_TYPE, 1),
            ("pages-repeat-leaf", tags::PAGE_REPEAT, 2),
            ("pages-repeat-branch", tags::PAGE_REPEAT, 2),
            ("pages-cycle-usable", tags::PAGE_CYCLE, 1),
            ("pages-parent-mismatch", tags::PAGE_PARENT, 1),
            ("pages-template-kid", tags::PAGE_TYPE_INVALID, 1),
            ("pages-direct-kids", tags::PAGE_DIRECT, 2),
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(strict.pages().unwrap_err().repair, Some(tag), "{name}");
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            let pages = document.pages().unwrap();
            assert_eq!(pages.len(), count, "{name}");
            assert_eq!(document.original_bytes(), bytes);
            let letter = matches!(name, "pages-media-missing" | "pages-media-empty");
            let expected = Rect {
                x0: 0.0,
                y0: 0.0,
                x1: if letter { 612.0 } else { 64.0 },
                y1: if letter { 792.0 } else { 64.0 },
            };
            assert_eq!(pages[0].media_box(), expected, "{name}");
            assert_eq!(pages[0].crop_box(), expected, "{name}");
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .any(|d| d.repair() == Some(tag))
            );
        }
    }

    fn preloaded(name: &str, bytes: usize) -> Document {
        let document = Document::open(
            fixture("document", &format!("{name}.pdf")),
            &OpenOptions {
                limits: Limits {
                    document_bytes: bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        for id in document.object_ids() {
            document.object(id).unwrap();
        }
        document
    }

    #[test]
    fn recovered_pages_use_one_cache_and_transactional_memory_reservations() {
        for name in [
            "pages-repeat-leaf",
            "pages-repeat-branch",
            "pages-cycle-usable",
            "pages-direct-kids",
            "pages-root-untyped-backlinks",
            "pages-media-missing",
            "xref-swapped-offsets",
            "object-stream-type-invalid",
            "object-stream-self-entry",
            "direct-stream-array",
            "direct-stream-indirect-array",
            "direct-stream-direct-page",
            "direct-stream-indirect-kids",
            "direct-stream-resources",
            "direct-stream-encrypted",
            "security-length-missing",
            "security-zero-padded-40",
            "security-zero-padded-48",
            "security-legacy-permission-selector",
            "catalog-number-zero",
        ] {
            let document = preloaded(name, 128 * 1024 * 1024);
            let initial = document.retained.used();
            let pages = document.pages().unwrap();
            let count = pages.len();
            let retained = document.retained.used();
            let mut unique = Vec::new();
            let mut expected = 0;
            for page in pages {
                expected += size_of::<super::Page>() + page.dictionary.retained_bytes()
                    - size_of::<pagura_core::Dictionary>()
                    + page
                        .source_path
                        .iter()
                        .map(pagura_core::syntax::PathComponent::retained_bytes)
                        .sum::<usize>();
                if !unique
                    .iter()
                    .any(|resource| Arc::ptr_eq(resource, &page.resources))
                {
                    unique.push(Arc::clone(&page.resources));
                    expected += page.resources.retained_bytes() + 2 * size_of::<usize>();
                }
            }
            assert_eq!(retained, initial + expected, "{name}");
            assert!(retained > initial, "{name}");
            document.pages().unwrap();
            assert_eq!(document.retained.used(), retained, "{name}");
            let exact = preloaded(name, retained);
            assert_eq!(exact.page_count().unwrap(), count, "{name}");
            assert_eq!(exact.retained.used(), retained, "{name}");
            let limited = preloaded(name, retained - 1);
            let error = limited.pages().unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit, "{name}");
            assert!(error.object.is_some());
            assert_eq!(limited.retained.used(), initial, "{name}");
            assert_eq!(limited.pages().unwrap_err(), error);
        }
        let repeated = preloaded("pages-repeat-leaf", usize::MAX);
        let pages = repeated.pages().unwrap();
        assert_eq!(pages[0].id(), ObjectId::new(3, 0));
        assert_eq!(pages[1].id(), pages[0].id());
        assert_eq!(pages[1].index(), 1);
        assert!(Arc::ptr_eq(&pages[0].resources, &pages[1].resources));
        let direct = preloaded("pages-direct-kids", usize::MAX);
        let pages = direct.pages().unwrap();
        assert_eq!(pages[0].id(), ObjectId::new(2, 0));
        assert!(!Arc::ptr_eq(&pages[0].resources, &pages[1].resources));
        assert!(pages[0].resources().contains_key(b"ProcSet"));
        assert!(pages[1].resources().contains_key(b"ExtGState"));
    }
    #[test]
    fn expanded_page_graphs_obey_work_and_page_limits_without_partial_results() {
        for work in [true, false] {
            let mut document = preloaded("pages-repeat-branch", usize::MAX);
            let initial = document.retained.used();
            if work {
                document.options.limits.value_nodes = 4;
            } else {
                document.options.limits.pages = 1;
            }
            let error = document.pages().unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert!(error.object.is_some());
            assert_eq!(document.retained.used(), initial);
            assert_eq!(document.pages().unwrap_err(), error);
        }
        let strict = Document::open(
            fixture("document", "xref-swapped-offsets.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(strict.repair, Some(tags::XREF_REBUILT));
        for (name, tag) in [
            ("object-stream-type-invalid", tags::OBJECT_STREAM_TYPE),
            ("object-stream-self-entry", tags::OBJECT_STREAM_SELF_ENTRY),
        ] {
            let error = Document::open(
                fixture("document", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.repair, Some(tag));
        }
    }
}
