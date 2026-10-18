//! One traversal for name and number trees (7.9.6–7.9.7, Tables 36–37).
use crate::view::{Context, Source, identity};
use crate::{Document, tags};
use pagura_core::{Error, ErrorKind, Result, Value};
use std::{collections::BTreeSet, fmt::Debug, marker::PhantomData};

mod sealed {
    pub trait Sealed {}
    impl Sealed for &[u8] {}
    impl Sealed for i64 {}
}
/// The two PDF tree key kinds. Sealed to byte strings and integers.
pub trait TreeKey<'a>: sealed::Sealed + Ord + Copy + Debug {
    /// Borrowed lookup-key type, independent of the tree lifetime.
    type Query: ?Sized;
    /// Compares a stored key with a borrowed query.
    fn compare(self, query: &Self::Query) -> std::cmp::Ordering;
    /// The leaf entry name: Names or Nums.
    const ENTRY: &'static [u8];
    /// Reads a direct key of the corresponding PDF type.
    fn from_value(value: &'a Value) -> Option<Self>;
}
impl<'a> TreeKey<'a> for &'a [u8] {
    type Query = [u8];
    fn compare(self, query: &[u8]) -> std::cmp::Ordering {
        self.cmp(query)
    }
    const ENTRY: &'static [u8] = b"Names";
    fn from_value(value: &'a Value) -> Option<Self> {
        value.as_string()
    }
}
impl TreeKey<'_> for i64 {
    type Query = i64;
    fn compare(self, query: &i64) -> std::cmp::Ordering {
        self.cmp(query)
    }
    const ENTRY: &'static [u8] = b"Nums";
    fn from_value(value: &Value) -> Option<Self> {
        value.as_integer()
    }
}
/// A string-keyed name tree; comparisons use decoded bytes, not text collation.
pub type NameTree<'a> = Tree<'a, &'a [u8]>;
/// An integer-keyed number tree.
pub type NumberTree<'a> = Tree<'a, i64>;
/// A lazy handle to a PDF name or number tree. No objects load on construction.
#[derive(Debug, Clone, Copy)]
pub struct Tree<'a, K> {
    doc: &'a Document,
    root: &'a Value,
    source: Source,
    kind: PhantomData<K>,
}
impl<'a, K: TreeKey<'a>> Tree<'a, K> {
    pub(super) fn new(doc: &'a Document, root: &'a Value) -> Self {
        Self {
            doc,
            root,
            source: Source::default(),
            kind: PhantomData,
        }
    }
    pub(super) fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }
    /// Looks up a key using child Limits to guide descent. Only visited nodes
    /// are validated on a hit. On a miss, a bounded full validation recovers
    /// keys hidden by damaged Limits, rather than returning a false absence.
    ///
    /// # Errors
    /// Returns positioned structural/reference/budget errors; safe tree repairs
    /// reject strictly with the corresponding `tree.*` repair tag.
    pub fn get(&self, key: &K::Query) -> Result<Option<&'a Value>> {
        self.get_with(&mut Context::new(self.doc), key)
            .map(|v| v.map(|(v, _)| v))
    }
    /// Validates the complete tree, then iterates in key order. Duplicate keys
    /// keep their first physical occurrence, with a reported `tree.order` repair.
    /// Values remain borrowed and unresolved.
    ///
    /// # Errors
    /// As [`Self::get`]. No partial collection is returned on a budget failure.
    pub fn entries(&self) -> Result<TreeEntries<'a, K>> {
        Ok(TreeEntries {
            inner: self.entries_with(&mut Context::new(self.doc))?.into_iter(),
        })
    }
    fn bounds(
        ctx: &mut Context<'a>,
        value: &'a Value,
        source: Source,
        root: bool,
    ) -> Result<Option<(K, K)>> {
        let (dictionary, source) = ctx.dictionary(value, source)?;
        let (limits, present) = match ctx.entry(dictionary, b"Limits", source) {
            Ok(value) => (value, value.is_some()),
            Err(error) if matches!(error.kind, ErrorKind::MissingObject | ErrorKind::Cycle) => {
                (None, true)
            }
            Err(error) => return Err(error),
        };
        let bounds = match limits.and_then(Value::as_array) {
            Some([low, high]) => Self::bound_key(ctx, low, source)?
                .zip(Self::bound_key(ctx, high, source)?)
                .filter(|(low, high)| low <= high),
            _ => None,
        };
        if (root && present) || (!root && bounds.is_none()) {
            ctx.repair(
                tags::TREE_LIMITS,
                "tree Limits absent or malformed; actual keys used (7.9.6–7.9.7)",
                source,
            )?;
        }
        Ok(bounds)
    }
    fn bound_key(ctx: &mut Context<'a>, value: &'a Value, source: Source) -> Result<Option<K>> {
        match ctx.resolve(value, source) {
            Ok((value, _)) => Ok(K::from_value(value)),
            Err(error) if matches!(error.kind, ErrorKind::MissingObject | ErrorKind::Cycle) => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    fn node(
        ctx: &mut Context<'a>,
        value: &'a Value,
        source: Source,
        root: bool,
    ) -> Result<Option<(&'a Value, Source)>> {
        match ctx.resolve(value, source) {
            Ok((value, _)) if !root && value.is_null() => {
                ctx.repair(
                    tags::TREE_STRUCTURE,
                    "null tree child omitted (7.9.6–7.9.7)",
                    source,
                )?;
                Ok(None)
            }
            Ok(value) => Ok(Some(value)),
            Err(error)
                if !root && matches!(error.kind, ErrorKind::MissingObject | ErrorKind::Cycle) =>
            {
                let tag = if error.kind == ErrorKind::Cycle {
                    tags::TREE_CYCLE
                } else {
                    tags::TREE_STRUCTURE
                };
                ctx.repair(tag, "unresolvable tree child omitted (7.9.6–7.9.7)", source)?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    pub(super) fn get_with(
        &self,
        ctx: &mut Context<'a>,
        key: &K::Query,
    ) -> Result<Option<(&'a Value, Source)>> {
        let mut stack = vec![(self.root, self.source, 0)];
        let mut visited = BTreeSet::new();
        while let Some((value, source, depth)) = stack.pop() {
            ctx.step(depth, source)?;
            let Some((value, source)) = Self::node(ctx, value, source, depth == 0)? else {
                continue;
            };
            ctx.allocate::<usize>(8, source)?;
            if !visited.insert(identity(value)) {
                ctx.repair(
                    tags::TREE_CYCLE,
                    "repeated tree node omitted (7.9.6–7.9.7)",
                    source,
                )?;
                continue;
            }
            let (dictionary, source) = ctx.dictionary(value, source)?;
            let bounds = Self::bounds(ctx, value, source, depth == 0)?;
            let pairs = ctx.entry(dictionary, K::ENTRY, source)?;
            let kids = ctx.entry(dictionary, b"Kids", source)?;
            match (pairs, kids) {
                (Some(pairs), None) => {
                    let entries = Self::pairs(ctx, pairs, source)?;
                    let actual = entries
                        .iter()
                        .map(|e| e.key)
                        .min()
                        .zip(entries.iter().map(|e| e.key).max());
                    if bounds.is_some() && bounds != actual {
                        ctx.repair(
                            tags::TREE_LIMITS,
                            "tree Limits disagree with leaf keys; actual keys used (7.9.6–7.9.7)",
                            source,
                        )?;
                    }
                    if let Some(entry) = entries
                        .iter()
                        .find(|e| e.key.compare(key) == std::cmp::Ordering::Equal)
                    {
                        return Ok(Some((entry.value, entry.source)));
                    }
                }
                (None, Some(kids)) => {
                    let kids = kids
                        .as_array()
                        .ok_or_else(|| source.error(Error::syntax("tree Kids must be an array")))?;
                    for child in kids.iter().rev() {
                        if !matches!(child, Value::Reference(_)) {
                            ctx.repair(
                                tags::TREE_STRUCTURE,
                                "direct tree child dictionary accepted (7.9.6–7.9.7)",
                                source,
                            )?;
                        }
                        let Some((child_value, child_source)) =
                            Self::node(ctx, child, source, false)?
                        else {
                            continue;
                        };
                        if visited.contains(&identity(child_value)) {
                            ctx.repair(
                                tags::TREE_CYCLE,
                                "repeated tree node omitted (7.9.6–7.9.7)",
                                child_source,
                            )?;
                            continue;
                        }
                        let bounds = Self::bounds(ctx, child_value, child_source, false)?;
                        if bounds.is_none_or(|(low, high)| {
                            low.compare(key) != std::cmp::Ordering::Greater
                                && high.compare(key) != std::cmp::Ordering::Less
                        }) {
                            ctx.allocate::<(&Value, Source, usize)>(1, source)?;
                            stack.push((child, source, depth + 1));
                        }
                    }
                }
                (Some(_), Some(_)) => {
                    return Ok(self
                        .entries_with(ctx)?
                        .into_iter()
                        .find(|e| e.key.compare(key) == std::cmp::Ordering::Equal)
                        .map(|e| (e.value, e.source)));
                }
                _ => {
                    return Err(source.error(Error::syntax(
                        "tree node requires exactly one of Kids and entries",
                    )));
                }
            }
        }
        Ok(self
            .entries_with(ctx)?
            .into_iter()
            .find(|e| e.key.compare(key) == std::cmp::Ordering::Equal)
            .map(|e| (e.value, e.source)))
    }
    fn pairs(ctx: &mut Context<'a>, value: &'a Value, source: Source) -> Result<Vec<Entry<'a, K>>> {
        let values = value
            .as_array()
            .ok_or_else(|| source.error(Error::syntax("tree entries must be an array")))?;
        if values.len() % 2 != 0 {
            return Err(source.error(Error::syntax("tree has an unmatched key")));
        }
        ctx.allocate::<Entry<'a, K>>(values.len() / 2, source)?;
        let entries: Vec<_> = values
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let (key, _) = ctx.resolve(&pair[0], source)?;
                K::from_value(key)
                    .map(|key| Entry {
                        key,
                        value: &pair[1],
                        source,
                    })
                    .ok_or_else(|| source.error(Error::syntax("invalid tree key type")))
            })
            .collect::<Result<_>>()?;
        if entries.windows(2).any(|v| v[0].key >= v[1].key) {
            ctx.repair(
                tags::TREE_ORDER,
                "tree keys reordered; first duplicate retained (7.9.6–7.9.7)",
                source,
            )?;
        }
        Ok(entries)
    }
    pub(super) fn entries_with(&self, ctx: &mut Context<'a>) -> Result<Vec<Entry<'a, K>>> {
        let mut stack = vec![Frame::Enter(self.root, self.source, 0)];
        let mut ranges: Vec<Option<(K, K)>> = Vec::new();
        let mut visited = BTreeSet::new();
        let mut entries = Vec::new();
        let mut tree_source = self.source;
        while let Some(frame) = stack.pop() {
            let (value, source, depth) = match frame {
                Frame::Exit(bounds, source) => {
                    let actual = ranges.pop().flatten();
                    if bounds.is_some() && bounds != actual {
                        ctx.repair(tags::TREE_LIMITS,"tree Limits disagree with descendant keys; actual keys used (7.9.6–7.9.7)",source)?;
                    }
                    if let (Some(parent), Some(actual)) = (ranges.last_mut(), actual) {
                        merge(parent, actual);
                    }
                    continue;
                }
                Frame::Enter(value, source, depth) => (value, source, depth),
            };
            ctx.step(depth, source)?;
            let Some((value, source)) = Self::node(ctx, value, source, depth == 0)? else {
                continue;
            };
            ctx.allocate::<usize>(8, source)?;
            if !visited.insert(identity(value)) {
                ctx.repair(
                    tags::TREE_CYCLE,
                    "repeated tree node omitted (7.9.6–7.9.7)",
                    source,
                )?;
                continue;
            }
            if depth == 0 {
                tree_source = source;
            }
            let (dictionary, source) = ctx.dictionary(value, source)?;
            let bounds = Self::bounds(ctx, value, source, depth == 0)?;
            ctx.allocate::<Frame<'a, K>>(1, source)?;
            ctx.allocate::<Option<(K, K)>>(1, source)?;
            stack.push(Frame::Exit(bounds, source));
            ranges.push(None);
            let pairs = ctx.entry(dictionary, K::ENTRY, source)?;
            let kids = ctx.entry(dictionary, b"Kids", source)?;
            if pairs.is_some() && kids.is_some() {
                ctx.repair(
                    tags::TREE_STRUCTURE,
                    "mixed tree node read as local entries then children (7.9.6–7.9.7)",
                    source,
                )?;
            }
            if pairs.is_none() && kids.is_none() {
                return Err(source.error(Error::syntax("tree node requires Kids or entries")));
            }
            if let Some(pairs) = pairs {
                let pairs = Self::pairs(ctx, pairs, source)?;
                if let Some(range) = ranges.last_mut() {
                    for entry in &pairs {
                        ctx.step(depth, source)?;
                        merge(range, (entry.key, entry.key));
                    }
                }
                entries.extend(pairs);
            }
            if let Some(kids) = kids {
                let kids = kids
                    .as_array()
                    .ok_or_else(|| source.error(Error::syntax("tree Kids must be an array")))?;
                ctx.allocate::<Frame<'a, K>>(kids.len(), source)?;
                for child in kids.iter().rev() {
                    if !matches!(child, Value::Reference(_)) {
                        ctx.repair(
                            tags::TREE_STRUCTURE,
                            "direct tree child dictionary accepted (7.9.6–7.9.7)",
                            source,
                        )?;
                    }
                    stack.push(Frame::Enter(child, source, depth + 1));
                }
            }
        }
        if entries.windows(2).any(|v| v[0].key >= v[1].key) {
            ctx.repair(
                tags::TREE_ORDER,
                "tree keys reordered; first duplicate retained (7.9.6–7.9.7)",
                tree_source,
            )?;
            ctx.allocate::<Entry<'a, K>>(entries.len(), tree_source)?;
            entries.sort_by_key(|entry| entry.key);
            entries.dedup_by_key(|entry| entry.key);
        }
        Ok(entries)
    }
}
enum Frame<'a, K> {
    Enter(&'a Value, Source, usize),
    Exit(Option<(K, K)>, Source),
}
fn merge<K: Ord + Copy>(range: &mut Option<(K, K)>, (low, high): (K, K)) {
    *range = Some(match *range {
        Some((a, b)) => (a.min(low), b.max(high)),
        None => (low, high),
    });
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Entry<'a, K> {
    pub key: K,
    pub value: &'a Value,
    pub source: Source,
}
/// Validated entries in ascending key order, borrowing their source values.
#[derive(Debug, Clone)]
pub struct TreeEntries<'a, K> {
    inner: std::vec::IntoIter<Entry<'a, K>>,
}
impl<'a, K> Iterator for TreeEntries<'a, K> {
    type Item = (K, &'a Value);
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|e| (e.key, e.value))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl<K> ExactSizeIterator for TreeEntries<'_, K> {}
impl<K> std::iter::FusedIterator for TreeEntries<'_, K> {}
