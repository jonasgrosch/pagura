//! Iterative, depth- and allocation-bounded structure hierarchy traversal.
use super::{Budget, Node, Plan, Reference, StructureElement};
use pagura_core::{Dictionary, Error, Name, ObjectId, Result, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;
use std::collections::BTreeSet;

/// Work of the iterative walk: a `/K` value to visit, or the end of an
/// element's descendants.
enum Task<'a> {
    Value {
        value: &'a Value,
        parent: Option<usize>,
        depth: usize,
    },
    End(usize),
}
pub(super) fn read<'a>(
    doc: &'a Document,
    root: &'a Dictionary,
    page: ObjectId,
    budget: &mut Budget,
) -> ReadResult<Plan<'a>> {
    let types = super::roles::Types::new(doc, root, budget)?;
    let mut plan = Plan {
        tagged: tagged(doc)?,
        nodes: Vec::new(),
        items: Vec::new(),
    };
    let mut tasks = Vec::new();
    if let Some(kids) = super::entry(doc, root, b"K")? {
        tasks.push(Task::Value {
            value: kids,
            parent: None,
            depth: 0,
        });
    }
    let mut seen = BTreeSet::new();
    while let Some(task) = tasks.pop() {
        budget.visit()?;
        let (value, parent, depth) = match task {
            Task::End(index) => {
                plan.nodes[index].end = plan.nodes.len();
                continue;
            }
            Task::Value {
                value,
                parent,
                depth,
            } => (value, parent, depth),
        };
        budget.depth(depth)?;
        let resolved = doc.resolve(value)?;
        if let Some(array) = resolved.as_array() {
            budget.store(array.len().saturating_mul(size_of::<Task<'_>>()))?;
            tasks.extend(array.iter().rev().map(|value| Task::Value {
                value,
                parent,
                depth: depth + 1,
            }));
        } else if let Some(mcid) = resolved.as_integer() {
            let parent = parent.ok_or(Error::syntax("structure root child is not an element"))?;
            let pg = page_id(doc, plan.nodes[parent].dictionary, None)?;
            marked(&mut plan, parent, pg, pg, mcid, page, budget)?;
        } else {
            let dictionary = resolved
                .as_dictionary()
                .ok_or(Error::syntax("invalid structure child"))?;
            match super::name(doc, dictionary, b"Type")?.map(Name::as_bytes) {
                Some(b"MCR" | b"OBJR") => {
                    content(&mut plan, doc, dictionary, parent, page, budget)?;
                }
                Some(b"StructElem") | None => {
                    if !seen.insert(std::ptr::from_ref(dictionary).addr()) {
                        return Err(
                            Error::cycle("repeated or cyclic structure element (14.7.2)").into(),
                        );
                    }
                    let expected = parent.map_or(root, |i| plan.nodes[i].dictionary);
                    let actual = dictionary
                        .get(b"P")
                        .filter(|v| v.as_reference().is_some())
                        .ok_or(Error::syntax(
                            "structure element P must be an indirect reference",
                        ))?;
                    if !std::ptr::eq(doc.resolve_dictionary(actual)?, expected) {
                        return Err(Error::syntax(
                            "structure element parent disagrees with K (14.7.2)",
                        )
                        .into());
                    }
                    let index = element(
                        &mut plan,
                        doc,
                        dictionary,
                        value.as_reference(),
                        parent,
                        &types,
                        budget,
                    )?;
                    tasks.push(Task::End(index));
                    if let Some(kids) = super::entry(doc, dictionary, b"K")? {
                        tasks.push(Task::Value {
                            value: kids,
                            parent: Some(index),
                            depth: depth + 1,
                        });
                    }
                }
                _ => return Err(Error::syntax("invalid structure child Type").into()),
            }
        }
    }
    Ok(plan)
}
fn element<'a>(
    plan: &mut Plan<'a>,
    doc: &'a Document,
    dictionary: &'a Dictionary,
    object: Option<ObjectId>,
    parent: Option<usize>,
    types: &super::roles::Types<'_>,
    budget: &mut Budget,
) -> ReadResult<usize> {
    let role = dictionary
        .get(b"S")
        .map(|v| doc.resolve(v))
        .transpose()?
        .and_then(Value::as_name)
        .ok_or(Error::syntax("structure element S must be a name"))?;
    let (namespace, standard_type) = types.resolve(doc, dictionary, role, budget)?;
    let actual = property(doc, dictionary, object, b"ActualText")?;
    let alt = property(doc, dictionary, object, b"Alt")?;
    let replacement = actual
        .map(|text| super::ReplacementText {
            text,
            kind: crate::layout::Replacement::Actual,
        })
        .or_else(|| {
            alt.map(|text| super::ReplacementText {
                text,
                kind: crate::layout::Replacement::Alt,
            })
        });
    budget.store(
        size_of::<Node<'_>>()
            + role.len() * 2
            + replacement.as_ref().map_or(0, |r| r.text.capacity()),
    )?;
    if plan.nodes.len() >= budget.limits.text_items {
        return Err(Error::limit("structure element count exceeded").into());
    }
    let artifact = standard_type
        .as_ref()
        .is_some_and(|n| n.as_bytes() == b"Artifact")
        || parent.is_some_and(|p| plan.nodes[p].artifact);
    let index = plan.nodes.len();
    plan.nodes.push(Node {
        dictionary,
        element: StructureElement {
            object,
            parent,
            role: role.clone(),
            standard_type,
            namespace,
            range: 0..0,
        },
        end: index + 1,
        replacement,
        artifact,
        relevant: false,
        other_page: false,
    });
    Ok(index)
}
fn content(
    plan: &mut Plan<'_>,
    doc: &Document,
    dictionary: &Dictionary,
    parent: Option<usize>,
    page: ObjectId,
    budget: &mut Budget,
) -> ReadResult<()> {
    let parent = parent.ok_or(Error::syntax("structure root child is not an element"))?;
    let pg = page_id(
        doc,
        dictionary,
        super::entry(doc, plan.nodes[parent].dictionary, b"Pg")?,
    )?;
    if super::name(doc, dictionary, b"Type")?.is_some_and(|n| n.as_bytes() == b"MCR") {
        let container = super::entry(doc, dictionary, b"Stm")?
            .map(|v| {
                v.as_reference()
                    .ok_or(Error::syntax("MCR Stm must be an indirect reference"))
            })
            .transpose()?
            .unwrap_or(pg);
        let mcid = dictionary
            .get(b"MCID")
            .map(|v| doc.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("MCR MCID must be an integer"))?;
        marked(plan, parent, pg, container, mcid, page, budget)
    } else {
        let object = dictionary
            .get(b"Obj")
            .and_then(Value::as_reference)
            .ok_or(Error::syntax("OBJR Obj must be an indirect reference"))?;
        if pg == page {
            item(plan, parent, Reference::Object(object), budget)?;
        } else {
            plan.nodes[parent].other_page = true;
        }
        Ok(())
    }
}
fn page_id(
    doc: &Document,
    dictionary: &Dictionary,
    fallback: Option<&Value>,
) -> ReadResult<ObjectId> {
    let id = super::entry(doc, dictionary, b"Pg")?
        .or(fallback)
        .and_then(Value::as_reference)
        .ok_or(Error::syntax(
            "structure content requires an indirect Pg (14.7.5)",
        ))?;
    let page = doc
        .object(id)?
        .dictionary()
        .ok_or(Error::syntax("structure Pg is not a dictionary"))?;
    if super::name(doc, page, b"Type")?.map(Name::as_bytes) != Some(b"Page") {
        return Err(Error::syntax("structure Pg does not identify a page").into());
    }
    Ok(id)
}
fn marked(
    plan: &mut Plan<'_>,
    node: usize,
    pg: ObjectId,
    container: ObjectId,
    mcid: i64,
    page: ObjectId,
    budget: &mut Budget,
) -> Result<()> {
    if mcid < 0 {
        return Err(Error::syntax("negative structure MCID"));
    }
    if pg == page {
        item(plan, node, Reference::Marked(container, mcid), budget)?;
    } else {
        plan.nodes[node].other_page = true;
    }
    Ok(())
}
fn item(plan: &mut Plan<'_>, node: usize, reference: Reference, budget: &mut Budget) -> Result<()> {
    budget.store(size_of::<super::Item>())?;
    plan.items.push(super::Item {
        node,
        reference,
        ranges: Vec::new(),
        visible: false,
    });
    plan.nodes[node].relevant = true;
    Ok(())
}
fn property(
    doc: &Document,
    dictionary: &Dictionary,
    object: Option<ObjectId>,
    key: &[u8],
) -> ReadResult<Option<String>> {
    super::entry(doc, dictionary, key)?
        .map(|v| {
            let bytes = doc
                .resolve(v)?
                .as_string()
                .ok_or(Error::syntax("structure replacement must be a text string"))?;
            super::text_string(doc, bytes, object)
        })
        .transpose()
}

pub(super) fn tagged(doc: &Document) -> ReadResult<bool> {
    let Some(value) = super::entry(doc, doc.catalog()?, b"MarkInfo")? else {
        return Ok(false);
    };
    let dictionary = doc.resolve_dictionary(value)?;
    super::entry(doc, dictionary, b"Marked")?
        .map(|v| {
            doc.resolve(v).and_then(|v| {
                v.as_bool()
                    .ok_or(Error::syntax("MarkInfo Marked must be boolean"))
            })
        })
        .transpose()
        .map(|v| v.unwrap_or(false))
}
