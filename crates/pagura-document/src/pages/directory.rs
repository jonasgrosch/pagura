//! One retained page-tree traversal for complete enumeration and requested pages.
//! Frames contain source addresses, never references into their owning Document.

use super::{Page, branch, geometry};
use crate::memory::{Reservation, path_bytes};
use crate::source::{NeedData, ReadFault, ReadResult};
use crate::view::ValueIdentity;
use crate::{Document, repairs};
use pagura_core::syntax::PathComponent;
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Meter, Name, ObjectId, Rotation, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

const ARC_COUNTS: usize = 2 * size_of::<usize>();

/// Existing borrowed enumeration drives exactly the same retained cursor.
pub(crate) fn read(document: &Document) -> ReadResult<(Vec<Page>, Reservation)> {
    let mut cursor = Cursor::new(document, Selection::All)?;
    while !cursor.advance(document)? {}
    document.check_source()?;
    let finished = cursor.finish()?;
    document.report(finished.diagnostics);
    Ok((finished.pages, finished.reservation))
}

/// A direct value's address in its containing indirect object. Dictionary paths
/// preserve repaired direct Kids occurrence identity without copying PDF values.
#[derive(Debug, Clone)]
struct Address {
    object: ObjectId,
    path: Vec<PathComponent>,
}

impl Address {
    fn indirect(object: ObjectId) -> Arc<Self> {
        Arc::new(Self {
            object,
            path: Vec::new(),
        })
    }

    fn child(&self, part: PathComponent) -> Arc<Self> {
        let mut path = self.path.clone();
        path.push(part);
        Arc::new(Self {
            object: self.object,
            path,
        })
    }

    fn value<'a>(&self, document: &'a Document) -> ReadResult<&'a Value> {
        let mut value = document.object(self.object)?.value();
        for part in &self.path {
            value = match part {
                PathComponent::Key(key) => value
                    .as_dictionary()
                    .and_then(|value| value.get(key.as_bytes())),
                PathComponent::Index(index) => value.as_array().and_then(|value| value.get(*index)),
            }
            .ok_or_else(|| {
                Error::syntax("page-tree source path is not a value").in_object(self.object)
            })?;
        }
        Ok(value)
    }

    fn dictionary<'a>(&self, document: &'a Document) -> ReadResult<&'a Dictionary> {
        self.value(document)?.as_dictionary().ok_or_else(|| {
            Error::syntax("page tree node must be a dictionary")
                .in_object(self.object)
                .into()
        })
    }
}

/// Only owners of the four inherited attributes are retained between steps.
#[derive(Debug, Clone, Default)]
struct Inheritance {
    resources: Option<Arc<Address>>,
    media_box: Option<Arc<Address>>,
    crop_box: Option<Arc<Address>>,
    rotate: Option<Arc<Address>>,
}

impl Inheritance {
    fn extended(&self, owner: &Arc<Address>, dictionary: &Dictionary) -> Self {
        let nearest = |key: &[u8], prior: &Option<Arc<Address>>| {
            if dictionary.contains_key(key) {
                Some(Arc::clone(owner))
            } else {
                prior.clone()
            }
        };
        Self {
            resources: nearest(b"Resources", &self.resources),
            media_box: nearest(b"MediaBox", &self.media_box),
            crop_box: nearest(b"CropBox", &self.crop_box),
            rotate: nearest(b"Rotate", &self.rotate),
        }
    }

    fn read<'a>(&self, document: &'a Document) -> ReadResult<Inherited<'a>> {
        let entry = |owner: &Option<Arc<Address>>, key: &[u8]| -> ReadResult<_> {
            Ok(owner
                .as_ref()
                .map(|owner| owner.dictionary(document))
                .transpose()?
                .and_then(|dictionary| dictionary.get(key)))
        };
        Ok(Inherited {
            resources: entry(&self.resources, b"Resources")?,
            media_box: entry(&self.media_box, b"MediaBox")?,
            crop_box: entry(&self.crop_box, b"CropBox")?,
            rotate: entry(&self.rotate, b"Rotate")?,
        })
    }
}

#[derive(Clone, Copy)]
struct Inherited<'a> {
    resources: Option<&'a Value>,
    media_box: Option<&'a Value>,
    crop_box: Option<&'a Value>,
    rotate: Option<&'a Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResourceKey {
    Empty,
    Object(ObjectId),
    Direct(ValueIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Branch,
    Leaf,
}

#[derive(Debug, Clone)]
struct Entry {
    address: Arc<Address>,
    inherited: Inheritance,
    parent: Option<ObjectId>,
    depth: usize,
}

#[derive(Debug, Clone)]
enum Children {
    Array(Arc<Address>),
    Single(ObjectId),
    Backlinks(ObjectId),
}

#[derive(Debug, Clone)]
enum Task {
    Enter(Entry),
    Children(Branch),
}

#[derive(Debug, Clone)]
struct Branch {
    entry: Entry,
    children: Children,
    next: usize,
    before: usize,
    declared: Option<i64>,
}

struct Finished {
    pages: Vec<Page>,
    reservation: Reservation,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
enum Parents {
    #[default]
    Unread,
    Scanning {
        last: Option<ObjectId>,
        entries: BTreeMap<ObjectId, Vec<ObjectId>>,
    },
    Ready(BTreeMap<ObjectId, Vec<ObjectId>>),
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Selection {
    All,
    Page(usize),
}

#[derive(Debug)]
pub(super) struct Cursor {
    root: ObjectId,
    tasks: Vec<Task>,
    visited: BTreeSet<ObjectId>,
    active: BTreeSet<ObjectId>,
    parents: Parents,
    omitted: bool,
    work: Meter,
    pages: Vec<Page>,
    selection: Selection,
    count: usize,
    reported: Option<usize>,
    shared: BTreeMap<ResourceKey, Arc<Dictionary>>,
    diagnostics: Diagnostics,
    reservation: Reservation,
    waiting: Option<NeedData>,
}

impl Cursor {
    pub(super) fn new(document: &Document, selection: Selection) -> ReadResult<Self> {
        let root = document
            .catalog()?
            .get(b"Pages")
            .and_then(Value::as_reference)
            .ok_or(Error::syntax("catalog Pages must be an indirect reference"))?;
        Ok(Self {
            root,
            tasks: vec![Task::Enter(Entry {
                address: Address::indirect(root),
                inherited: Inheritance::default(),
                parent: None,
                depth: 0,
            })],
            visited: BTreeSet::new(),
            active: BTreeSet::new(),
            parents: Parents::Unread,
            omitted: false,
            work: Meter::new(
                document.limits().traversal_work,
                "page-tree traversal work budget exceeded",
            ),
            pages: Vec::new(),
            selection,
            count: 0,
            reported: None,
            shared: BTreeMap::new(),
            diagnostics: Diagnostics::new(document.recovery()),
            reservation: document.retained.reservation(),
            waiting: None,
        })
    }

    /// Completes one structural unit. An unchanged range wait does no work.
    pub(super) fn advance(&mut self, document: &Document) -> ReadResult<bool> {
        document.check_source()?;
        if let Some(missing) = &self.waiting
            && missing.generation() == document.bytes.generation()
        {
            return Err(ReadFault::Unavailable(missing.clone()));
        }
        self.waiting = None;
        let mut diagnostics = Diagnostics::new(document.recovery());
        match self.step(document, &mut diagnostics) {
            Ok(()) => {
                self.diagnostics.absorb(diagnostics.take())?;
                if self.tasks.is_empty() {
                    self.validate_complete()?;
                }
                Ok(self.tasks.is_empty())
            }
            Err(ReadFault::Unavailable(missing)) => {
                self.waiting = Some(missing.clone());
                Err(ReadFault::Unavailable(missing))
            }
            Err(error) => {
                self.diagnostics.absorb(diagnostics.take())?;
                Err(error)
            }
        }
    }

    pub(super) fn publish(&mut self, document: &Document) {
        document.report(self.diagnostics.take());
    }

    pub(super) fn work(&self) -> usize {
        self.work.used()
    }

    pub(super) fn reported_count(&self) -> Option<usize> {
        self.reported
    }

    pub(super) fn verified_count(&self) -> Option<usize> {
        self.tasks.is_empty().then_some(self.count)
    }

    /// Transfers the selected page and its exact retained charge to the caller.
    /// The remaining structural cursor can then be dropped without losing it.
    pub(super) fn take_page(&mut self, document: &Document) -> ReadResult<Option<Page>> {
        let Some(page) = self.pages.last() else {
            return Ok(None);
        };
        document.check_source()?;
        self.reservation.charge(
            ARC_COUNTS.saturating_add(size_of::<Reservation>()),
            page.id(),
        )?;
        let retained = std::mem::replace(&mut self.reservation, document.retained.reservation());
        let page = self.pages.pop().map(|mut page| {
            page._reservation = Some(Arc::new(retained));
            page
        });
        document.report(self.diagnostics.take());
        Ok(page)
    }

    fn step(&mut self, document: &Document, diagnostics: &mut Diagnostics) -> ReadResult<()> {
        if matches!(self.parents, Parents::Scanning { .. }) {
            return self.scan_parents(document);
        }
        match self.tasks.last().cloned() {
            Some(Task::Enter(entry)) => self.enter(document, diagnostics, entry),
            Some(Task::Children(branch)) => self.child(document, diagnostics, branch),
            None => Ok(()),
        }
    }

    fn enter(
        &mut self,
        document: &Document,
        diagnostics: &mut Diagnostics,
        mut entry: Entry,
    ) -> ReadResult<()> {
        let id = entry.address.object;
        let indirect = entry.address.path.is_empty();
        if indirect && self.active.contains(&id) {
            diagnostics.repair(
                Diagnostic::new(repairs::PAGE_CYCLE, "cyclic page-tree child edge omitted")
                    .in_object(id),
            )?;
            self.omitted = true;
            self.tasks.pop();
            return Ok(());
        }
        let dictionary = entry.address.dictionary(document)?;
        if indirect && self.visited.contains(&id) {
            diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_REPEAT,
                    "repeated page-tree node retained in Kids order",
                )
                .in_object(id),
            )?;
        }
        if entry.depth > document.limits().depth {
            return Err(Error::limit("page tree depth budget exceeded")
                .in_object(id)
                .into());
        }
        self.work.charge(1).map_err(|error| error.in_object(id))?;
        if dictionary
            .get(b"Parent")
            .is_some_and(|value| value.as_reference() != entry.parent)
        {
            diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_PARENT,
                    "Parent disagrees with traversed Kids edge; Kids ancestry used",
                )
                .in_object(id),
            )?;
        }
        entry.inherited = entry.inherited.extended(&entry.address, dictionary);
        let Some(kind) = self.classify(document, diagnostics, id, dictionary, entry.parent)? else {
            diagnostics.take();
            return Ok(());
        };
        match kind {
            NodeKind::Branch => {
                let Some(children) = self.children(document, diagnostics, &entry, dictionary)?
                else {
                    diagnostics.take();
                    return Ok(());
                };
                let declared = dictionary.get(b"Count").and_then(Value::as_integer);
                if entry.parent.is_none() {
                    self.reported = declared.and_then(|count| usize::try_from(count).ok());
                }
                if indirect {
                    self.visited.insert(id);
                    self.active.insert(id);
                }
                self.tasks.pop();
                self.tasks.push(Task::Children(Branch {
                    entry,
                    children,
                    next: 0,
                    before: self.count,
                    declared,
                }));
            }
            NodeKind::Leaf => {
                if self.count >= document.limits().pages {
                    return Err(Error::limit("page count budget exceeded")
                        .in_object(id)
                        .into());
                }
                if match self.selection {
                    Selection::All => true,
                    Selection::Page(index) => self.count == index,
                } {
                    let page = self.leaf(
                        document,
                        diagnostics,
                        id,
                        dictionary,
                        entry.inherited.read(document)?,
                        &entry.address.path,
                    )?;
                    self.pages.push(page);
                }
                if indirect {
                    self.visited.insert(id);
                }
                self.count += 1;
                self.tasks.pop();
            }
        }
        Ok(())
    }

    fn children(
        &mut self,
        document: &Document,
        diagnostics: &mut Diagnostics,
        entry: &Entry,
        dictionary: &Dictionary,
    ) -> ReadResult<Option<Children>> {
        let id = entry.address.object;
        let indirect = dictionary
            .get(b"Kids")
            .is_some_and(|value| matches!(value, Value::Reference(_)));
        let mut address = entry.address.child(PathComponent::Key(Name::new(b"Kids")));
        let mut value = dictionary.get(b"Kids");
        for _ in 0..=document.limits().depth {
            let Some(Value::Reference(reference)) = value else {
                break;
            };
            if !document.slots.contains_key(reference) {
                value = None;
                break;
            }
            address = Address::indirect(*reference);
            value = Some(address.value(document)?);
        }
        if value.is_some_and(|value| matches!(value, Value::Reference(_))) {
            return Err(Error::limit("Kids reference chain too long")
                .in_object(id)
                .into());
        }
        match value {
            Some(Value::Array(_)) => Ok(Some(Children::Array(address))),
            Some(Value::Dictionary(_)) if indirect => {
                diagnostics.repair(
                    Diagnostic::new(
                        repairs::PAGE_KIDS,
                        "single Kids reference treated as a one-element array",
                    )
                    .in_object(id),
                )?;
                Ok(Some(Children::Single(address.object)))
            }
            _ => {
                if !entry.address.path.is_empty() {
                    return Err(Error::syntax("direct Pages dictionary has no usable Kids")
                        .in_object(id)
                        .into());
                }
                let Some(kids) = self.backlinks(id) else {
                    return Ok(None);
                };
                if kids.is_empty() {
                    return Err(Error::syntax(
                        "Pages node has no Kids or explicit Parent backlinks",
                    )
                    .in_object(id)
                    .into());
                }
                diagnostics.repair(
                    Diagnostic::new(
                        repairs::PAGE_KIDS_REBUILT,
                        "Kids recovered from explicit Parent backlinks in object-number order",
                    )
                    .in_object(id),
                )?;
                Ok(Some(Children::Backlinks(id)))
            }
        }
    }

    fn child(
        &mut self,
        document: &Document,
        diagnostics: &mut Diagnostics,
        branch: Branch,
    ) -> ReadResult<()> {
        let Branch {
            entry,
            children,
            next,
            before,
            declared,
        } = branch;
        let id = entry.address.object;
        let (address, done) = match children {
            Children::Array(ref source) => {
                let kids = source
                    .value(document)?
                    .as_array()
                    .ok_or_else(|| Error::syntax("page-tree Kids changed type").in_object(id))?;
                match kids.get(next) {
                    None => (None, true),
                    Some(Value::Reference(kid)) if document.slots.contains_key(kid) => {
                        (Some(Address::indirect(*kid)), false)
                    }
                    Some(Value::Dictionary(_)) => {
                        diagnostics.repair(
                            Diagnostic::new(
                                repairs::PAGE_DIRECT,
                                "direct Kids dictionary used with its containing object's identity",
                            )
                            .in_object(id),
                        )?;
                        (Some(source.child(PathComponent::Index(next))), false)
                    }
                    Some(_) => {
                        diagnostics.repair(
                            Diagnostic::new(
                                repairs::PAGE_KIDS,
                                "invalid or undefined Kids entry omitted",
                            )
                            .in_object(id),
                        )?;
                        self.omitted = true;
                        (None, false)
                    }
                }
            }
            Children::Single(kid) => (
                if next == 0 {
                    Some(Address::indirect(kid))
                } else {
                    None
                },
                next != 0,
            ),
            Children::Backlinks(parent) => {
                let kid = self
                    .backlinks(parent)
                    .and_then(|kids| kids.get(next))
                    .copied();
                (kid.map(Address::indirect), kid.is_none())
            }
        };
        if done {
            let found = i64::try_from(self.count - before).unwrap_or(i64::MAX);
            if declared != Some(found) {
                diagnostics.repair(
                    Diagnostic::new(
                        repairs::PAGE_COUNT,
                        "Pages Count disagrees with its leaves; leaves used",
                    )
                    .in_object(id),
                )?;
            }
            if entry.address.path.is_empty() {
                self.active.remove(&id);
            }
            self.tasks.pop();
        } else {
            if let Some(Task::Children(Branch { next, .. })) = self.tasks.last_mut() {
                *next = next.saturating_add(1);
            }
            if let Some(address) = address {
                self.tasks.push(Task::Enter(Entry {
                    address,
                    inherited: entry.inherited,
                    parent: Some(id),
                    depth: entry.depth.checked_add(1).ok_or_else(|| {
                        Error::limit("page tree depth budget exceeded").in_object(id)
                    })?,
                }));
            }
        }
        Ok(())
    }

    /// Start the bounded fallback once, then resume one indexed object per step.
    fn backlinks(&mut self, parent: ObjectId) -> Option<&[ObjectId]> {
        if matches!(self.parents, Parents::Unread) {
            self.parents = Parents::Scanning {
                last: None,
                entries: BTreeMap::new(),
            };
        }
        match &self.parents {
            Parents::Ready(entries) => Some(entries.get(&parent).map_or(&[], Vec::as_slice)),
            _ => None,
        }
    }

    fn scan_parents(&mut self, document: &Document) -> ReadResult<()> {
        let Parents::Scanning { last, entries } = &mut self.parents else {
            return Ok(());
        };
        let bound = last.map_or(Unbounded, Excluded);
        let Some((&id, _)) = document.slots.range((bound, Unbounded)).next() else {
            self.parents = Parents::Ready(std::mem::take(entries));
            return Ok(());
        };
        self.work.charge(1).map_err(|error| error.in_object(id))?;
        let object = match document.object(id) {
            Ok(object) => Some(object),
            Err(ReadFault::Invalid(error)) if crate::xref::structural(&error) => None,
            Err(error) => return Err(error),
        };
        if let Some(dictionary) = object.and_then(crate::IndirectObject::dictionary)
            && (dictionary.is_name(b"Type", b"Page") || dictionary.is_name(b"Type", b"Pages"))
            && let Some(parent) = dictionary.get(b"Parent").and_then(Value::as_reference)
        {
            entries.entry(parent).or_default().push(id);
        }
        *last = Some(id);
        Ok(())
    }

    fn validate_complete(&self) -> ReadResult<()> {
        if self.count == 0 && self.omitted {
            let cyclic = self
                .diagnostics
                .as_slice()
                .iter()
                .any(|item| item.repair() == Some(repairs::PAGE_CYCLE));
            return Err(if cyclic {
                Error::cycle("cyclic page tree contains no usable leaves")
            } else {
                Error::syntax("damaged page tree contains no usable leaves")
            }
            .in_object(self.root)
            .into());
        }
        Ok(())
    }

    fn finish(mut self) -> ReadResult<Finished> {
        self.validate_complete()?;
        Ok(Finished {
            pages: self.pages,
            reservation: self.reservation,
            diagnostics: self.diagnostics.take(),
        })
    }
    /// Decides from its Type, or reports how it was inferred (7.7.3.2).
    fn classify(
        &mut self,
        document: &Document,
        diagnostics: &mut Diagnostics,
        id: ObjectId,
        dictionary: &Dictionary,
        parent: Option<ObjectId>,
    ) -> ReadResult<Option<NodeKind>> {
        if dictionary.is_name(b"Type", b"Pages") {
            return Ok(Some(NodeKind::Branch));
        }
        if dictionary.is_name(b"Type", b"Page") {
            return Ok(Some(NodeKind::Leaf));
        }
        if dictionary.is_name(b"Type", b"Template")
            && parent.is_some()
            && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"))
        {
            diagnostics.repair(
                Diagnostic::new(
                    repairs::PAGE_TYPE_INVALID,
                    "Template in Kids displayed as a Page",
                )
                .in_object(id),
            )?;
            return Ok(Some(NodeKind::Leaf));
        }
        if dictionary.contains_key(b"Type") {
            return Err(Error::syntax("page tree node Type must be Pages or Page")
                .in_object(id)
                .into());
        }
        // Leaf evidence comes from the node itself: an inherited MediaBox
        // would make every untyped, childless node look like a page.
        let has_kids = dictionary.contains_key(b"Kids");
        let leaf = !has_kids
            && (dictionary.contains_key(b"Contents") || dictionary.contains_key(b"MediaBox"));
        let backlinked = if !leaf && !has_kids {
            let Some(kids) = self.backlinks(id) else {
                return Ok(None);
            };
            !kids.is_empty()
        } else {
            false
        };
        let kind = if branch(document, dictionary)? || backlinked {
            NodeKind::Branch
        } else if leaf {
            NodeKind::Leaf
        } else {
            return Err(Error::syntax("page tree node has no identifiable Type")
                .in_object(id)
                .into());
        };
        diagnostics.repair(
            Diagnostic::new(
                repairs::PAGE_TYPE,
                "missing page-tree Type inferred from Kids or leaf attributes",
            )
            .at(document.object(id)?.source().position())
            .in_object(id),
        )?;
        Ok(Some(kind))
    }

    /// Resolves the inherited Resources entry once per resource dictionary.
    fn resources(
        &mut self,
        document: &Document,
        page: ObjectId,
        inherited: Inherited<'_>,
    ) -> ReadResult<Arc<Dictionary>> {
        let key = match inherited.resources {
            None => ResourceKey::Empty,
            Some(Value::Reference(reference)) => ResourceKey::Object(*reference),
            Some(value) => ResourceKey::Direct(ValueIdentity::of(value)),
        };
        if let Some(shared) = self.shared.get(&key) {
            return Ok(Arc::clone(shared));
        }
        let value = inherited
            .resources
            .map(|value| document.resolve(value))
            .transpose()?;
        let dictionary = match value {
            Some(Value::Dictionary(resources)) => Some(resources),
            None | Some(Value::Null) => None,
            _ => {
                return Err(Error::syntax("Resources must be a dictionary")
                    .in_object(page)
                    .into());
            }
        };
        let resources =
            dictionary.map_or_else(Arc::default, |dictionary| Arc::new(dictionary.clone()));
        self.reservation
            .charge(resources.retained_bytes().saturating_add(ARC_COUNTS), page)?;
        self.shared.insert(key, Arc::clone(&resources));
        Ok(resources)
    }

    fn leaf(
        &mut self,
        document: &Document,
        diagnostics: &mut Diagnostics,
        id: ObjectId,
        dictionary: &Dictionary,
        inherited: Inherited<'_>,
        source_path: &[PathComponent],
    ) -> ReadResult<Page> {
        let (media_box, crop_box) = geometry::boxes(
            document,
            id,
            inherited.media_box,
            inherited.crop_box,
            diagnostics,
        )?;
        let rotation = match inherited.rotate {
            None => Rotation::None,
            Some(value) => document
                .integer(value)
                .and_then(|number| Ok(Rotation::try_from(number)?))
                .map_err(|error| error.in_object(id))?,
        };
        let user_unit = match dictionary.get(b"UserUnit") {
            None => 1.0,
            Some(value) => {
                let unit = document
                    .number(value)
                    .map_err(|error| error.in_object(id))?;
                if unit <= 0.0 {
                    return Err(Error::syntax("UserUnit must be a positive number")
                        .in_object(id)
                        .into());
                }
                unit
            }
        };
        let resources = self.resources(document, id, inherited)?;
        let dictionary = dictionary.clone();
        let source_path = source_path.to_vec();
        let bytes = size_of::<Page>()
            .saturating_add(
                dictionary
                    .retained_bytes()
                    .saturating_sub(size_of::<Dictionary>()),
            )
            .saturating_add(path_bytes(&source_path, source_path.capacity()));
        self.reservation.charge(bytes, id)?;
        Ok(Page {
            document: Arc::downgrade(&document.token),
            id,
            index: self.count,
            media_box,
            crop_box,
            rotation,
            user_unit,
            resources,
            dictionary,
            source_path,
            _reservation: None,
        })
    }
}

#[cfg(test)]
mod tests;
