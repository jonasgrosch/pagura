//! Forward /K references and reverse ParentTree entries must agree (14.7.5.4).
use super::{
    BTreeMap, BTreeSet, Budget, Dictionary, Document, Error, Page, Plan, Range, Reference, Result,
    TextPage, Value,
};

pub(super) fn resolve(
    plan: &mut Plan<'_>,
    doc: &Document,
    root: &Dictionary,
    page: &Page,
    text: &TextPage,
    budget: &mut Budget,
) -> Result<()> {
    let Occurrences {
        mut marked,
        visible,
    } = occurrences(text, page, budget)?;
    let parents = super::entry(doc, root, b"ParentTree")?
        .map(|v| doc.number_tree(v).entries())
        .transpose()?;
    budget.store(
        parents
            .as_ref()
            .map_or(0, ExactSizeIterator::len)
            .saturating_mul(80),
    )?;
    if parents.is_none() && (!plan.items.is_empty() || plan.nodes.iter().any(|n| n.other_page)) {
        return Err(Error::syntax(
            "structure content requires ParentTree (Table 354)",
        ));
    }
    let parents: BTreeMap<_, _> = parents.into_iter().flatten().collect();
    budget.store(text.glyphs().len())?;
    let mut covered = vec![false; text.glyphs().len()];
    let mut references = BTreeSet::new();
    for item in &mut plan.items {
        budget.work(1)?;
        if !references.insert(item.reference) {
            return Err(Error::syntax(
                "repeated structure content reference (14.7.5)",
            ));
        }
        check_parent(
            doc,
            &parents,
            item.reference,
            plan.nodes[item.node].dictionary,
        )?;
        item.ranges = marked.remove(&item.reference).unwrap_or_default();
        item.visible = visible.get(&item.reference).copied().unwrap_or(true);
        if matches!(item.reference, Reference::Marked(..)) && item.ranges.is_empty() {
            if matches!(item.reference, Reference::Marked(owner, _) if owner != page.id()) {
                return Err(Error::unsupported(
                    super::CONTENT,
                    "text.structure-content: structure references an uncaptured content stream",
                ));
            }
            return Err(Error::syntax(
                "structure MCID has no marked-content occurrence",
            ));
        }
        for range in &item.ranges {
            let glyphs = covered
                .get_mut(range.clone())
                .ok_or(Error::invalid_argument("invalid extraction object range"))?;
            budget.work(glyphs.len())?;
            if glyphs.iter().any(|used| *used) {
                return Err(Error::syntax(
                    "overlapping structure content items (14.7.5.1)",
                ));
            }
            glyphs.fill(true);
        }
    }
    for (glyph, covered) in text.glyphs().iter().zip(covered) {
        budget.work(glyph.marked_content.len() + 1)?;
        if !covered
            && !glyph.hidden
            && !glyph
                .marked_content
                .iter()
                .any(|m| m.tag.as_bytes() == b"Artifact")
        {
            return Err(Error::syntax(
                "tagged page has text outside its logical structure (14.8.2.2)",
            ));
        }
    }
    for index in (0..plan.nodes.len()).rev() {
        if plan.nodes[index].other_page
            && let Some(parent) = plan.nodes[index].element.parent
        {
            plan.nodes[parent].other_page = true;
        }
        if plan.nodes[index].relevant
            && let Some(parent) = plan.nodes[index].element.parent
        {
            plan.nodes[parent].relevant = true;
        }
    }
    if plan.tagged
        && plan
            .nodes
            .iter()
            .any(|n| n.relevant && n.element.standard_type.is_none())
    {
        return Err(Error::syntax(
            "tagged structure role has no standard mapping (14.8.4)",
        ));
    }
    Ok(())
}
struct Occurrences {
    marked: BTreeMap<Reference, Vec<Range<usize>>>,
    visible: BTreeMap<Reference, bool>,
}
fn occurrences(text: &TextPage, page: &Page, budget: &mut Budget) -> Result<Occurrences> {
    let mut marked: BTreeMap<Reference, Vec<Range<usize>>> = BTreeMap::new();
    let mut visible = BTreeMap::new();
    for span in &text.positioned.marked {
        budget.work(span.ancestors.len() + 1)?;
        let Some(mcid) = span.content.mcid else {
            continue;
        };
        if span.ancestors.iter().any(|a| a.mcid.is_some()) {
            return Err(Error::syntax("nested structural content items (14.7.5.1)"));
        }
        let container = span
            .content
            .container
            .ok_or(Error::invalid_argument("MCID lacks its source container"))?;
        budget.store(2 * size_of::<Range<usize>>() + 192)?;
        let ranges = marked
            .entry(Reference::Marked(container, mcid))
            .or_default();
        if container == page.id() && !ranges.is_empty() {
            return Err(Error::syntax("duplicate page MCID (14.7.5.2)"));
        }
        ranges.push(span.glyphs.clone());
        *visible
            .entry(Reference::Marked(container, mcid))
            .or_insert(false) |= !span.hidden;
    }
    for object in &text.positioned.objects {
        *visible
            .entry(Reference::Object(object.object))
            .or_insert(false) |= (!object.hidden)
            && !object
                .marked_content
                .iter()
                .any(|m| m.tag.as_bytes() == b"Artifact");
        budget.store(2 * size_of::<Range<usize>>() + 192)?;
        marked
            .entry(Reference::Object(object.object))
            .or_default()
            .push(object.glyphs.clone());
    }
    Ok(Occurrences { marked, visible })
}
fn check_parent(
    doc: &Document,
    parents: &BTreeMap<i64, &Value>,
    reference: Reference,
    expected: &Dictionary,
) -> Result<()> {
    let (object, key) = match reference {
        Reference::Marked(id, _) => (id, b"StructParents".as_slice()),
        Reference::Object(id) => (id, b"StructParent".as_slice()),
    };
    let dictionary = doc
        .object(object)?
        .dictionary()
        .ok_or(Error::syntax("structure content owner is not a dictionary"))?;
    if super::entry(doc, dictionary, b"StructParent")?.is_some()
        && super::entry(doc, dictionary, b"StructParents")?.is_some()
    {
        return Err(Error::syntax(
            "StructParent and StructParents are mutually exclusive (Table 359)",
        ));
    }
    let key = dictionary
        .get(key)
        .map(|v| doc.resolve(v))
        .transpose()?
        .and_then(Value::as_integer)
        .ok_or(Error::syntax(
            "structure content has no valid parent-tree key",
        ))?;
    let value = parents
        .get(&key)
        .ok_or(Error::syntax("missing ParentTree entry (14.7.5.4)"))?;
    let value = if let Reference::Marked(_, mcid) = reference {
        let index = usize::try_from(mcid).map_err(|_| Error::syntax("invalid MCID index"))?;
        doc.resolve(value)?
            .as_array()
            .and_then(|a| a.get(index))
            .ok_or(Error::syntax("MCID outside ParentTree array (14.7.5.4)"))?
    } else {
        value
    };
    if value.as_reference().is_none() || !std::ptr::eq(doc.resolve_dictionary(value)?, expected) {
        return Err(Error::syntax(
            "ParentTree owner disagrees with structure K (14.7.5.4)",
        ));
    }
    Ok(())
}
