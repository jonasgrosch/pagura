//! Logical order comes from /K. Separators remain explicit reader policy:
//! paragraph/heading boundaries use two LFs, cells/items one LF; geometry supplies
//! spacing within blocks. Consecutive ActualText replacements have no break.
use super::{BTreeMap, Budget, Error, LogicalText, Plan, Range, Reference, Result, TextPage};
use crate::page::layout::{
    Atom, Replacement, atoms,
    copy::append,
    geometry::{gap, same_line},
};

pub(super) fn export(page: &TextPage, plan: &Plan<'_>, budget: &mut Budget) -> Result<LogicalText> {
    let groups = groups(page, plan, budget)?;
    let mut output = Output {
        page,
        logical: LogicalText {
            text: String::new(),
            characters: Vec::new(),
            structure: Vec::new(),
            retained: 0,
        },
        last: None,
        block: None,
        replacement: None,
    };
    let mut emitted = std::collections::BTreeSet::new();
    let mut ranges = vec![None; plan.nodes.len()];
    budget.store(
        ranges
            .len()
            .saturating_mul(size_of::<Option<Range<usize>>>()),
    )?;
    for (item, atoms) in plan.items.iter().zip(groups) {
        let node = &plan.nodes[item.node];
        if node.artifact || !item.visible {
            continue;
        }
        let mut replacement = None;
        let mut ancestor = Some(item.node);
        while let Some(index) = ancestor {
            budget.work(1)?;
            if plan.nodes[index].replacement.is_some() {
                replacement = Some(index);
            }
            ancestor = plan.nodes[index].element.parent;
        }
        if let Some(index) = replacement {
            if !emitted.insert(index) {
                continue;
            }
            let atom = replacement_atom(output.page, plan, index, budget)?;
            output.emit(&atom, block(plan, index), budget)?;
            update_ranges(
                plan,
                &mut ranges,
                index,
                output.logical.text.len() - encoded_len(&atom)..output.logical.text.len(),
                budget,
            )?;
        } else {
            let block = block(plan, item.node);
            let mut span: Option<Range<usize>> = None;
            for atom in atoms {
                output.emit(&atom, block, budget)?;
                let range =
                    output.logical.text.len() - encoded_len(&atom)..output.logical.text.len();
                span = Some(span.map_or_else(
                    || range.clone(),
                    |r| r.start.min(range.start)..r.end.max(range.end),
                ));
            }
            if let Some(span) = span {
                update_ranges(plan, &mut ranges, item.node, span, budget)?;
            }
        }
    }
    let mut elements = Vec::new();
    let mut indices = vec![None; plan.nodes.len()];
    for (index, node) in plan
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.relevant && !n.artifact)
    {
        let mut element = node.element.clone();
        element.parent = element.parent.and_then(|i| indices[i]);
        element.range = ranges[index].clone().unwrap_or_else(|| {
            // Replaced descendants have no independent text range.
            let mut parent = node.element.parent;
            while let Some(i) = parent {
                if let Some(range) = &ranges[i] {
                    return range.start..range.start;
                }
                parent = plan.nodes[i].element.parent;
            }
            0..0
        });
        indices[index] = Some(elements.len());
        elements.push(element);
    }
    output.logical.structure = elements;
    output.logical.retained = crate::page::memory::logical_bytes(&output.logical);
    Ok(output.logical)
}
fn encoded_len(atom: &Atom) -> usize {
    atom.chars
        .iter()
        .map(|c| c.unwrap_or('\u{fffd}').len_utf8())
        .sum()
}
fn update_ranges(
    plan: &Plan<'_>,
    ranges: &mut [Option<Range<usize>>],
    index: usize,
    range: Range<usize>,
    budget: &mut Budget,
) -> Result<()> {
    let mut index = Some(index);
    while let Some(i) = index {
        budget.work(1)?;
        ranges[i] = Some(ranges[i].as_ref().map_or_else(
            || range.clone(),
            |r| r.start.min(range.start)..r.end.max(range.end),
        ));
        index = plan.nodes[i].element.parent;
    }
    Ok(())
}
fn groups(page: &TextPage, plan: &Plan<'_>, budget: &mut Budget) -> Result<Vec<Vec<Atom>>> {
    budget.store(
        page.glyphs()
            .len()
            .saturating_mul(size_of::<Option<usize>>())
            .saturating_add(plan.items.len().saturating_mul(size_of::<Vec<Atom>>())),
    )?;
    let mut owners = vec![None; page.glyphs().len()];
    let mut by_reference = BTreeMap::new();
    for (index, item) in plan.items.iter().enumerate() {
        by_reference.insert(item.reference, index);
        for range in &item.ranges {
            budget.work(range.len())?;
            owners[range.clone()].fill(Some(index));
        }
    }
    budget.store(page.positioned.marked.len().saturating_mul(80))?;
    let occurrences: BTreeMap<_, _> = page
        .positioned
        .marked
        .iter()
        .map(|s| (s.content.id, s))
        .collect();
    let mut groups: Vec<Vec<Atom>> = (0..plan.items.len()).map(|_| Vec::new()).collect();
    for atom in atoms(
        &page.positioned,
        crate::LayoutOptions::default(),
        page.limits,
    )? {
        let owner = if let Some(first) = atom.geometry {
            if page.glyphs()[first]
                .marked_content
                .iter()
                .any(|m| m.tag.as_bytes() == b"Artifact")
            {
                continue;
            }
            let owner = owners[first];
            budget.work(atom.glyphs.len())?;
            if owners[atom.glyphs.clone()].iter().any(|n| *n != owner) {
                return Err(Error::unsupported(
                    super::REPLACEMENT,
                    "text.structure-replacement: marked replacement crosses logical content items",
                ));
            }
            owner
        } else {
            atom.occurrence
                .and_then(|i| occurrences.get(&i).copied())
                .and_then(|span| {
                    std::iter::once(span.content.as_ref())
                        .chain(span.ancestors.iter().rev().map(AsRef::as_ref))
                        .find_map(|m| {
                            m.container.zip(m.mcid).and_then(|(id, mcid)| {
                                by_reference.get(&Reference::Marked(id, mcid)).copied()
                            })
                        })
                })
        };
        if let Some(owner) = owner {
            groups[owner].push(atom);
        }
    }
    Ok(groups)
}
fn replacement_atom(
    page: &TextPage,
    plan: &Plan<'_>,
    index: usize,
    budget: &mut Budget,
) -> Result<Atom> {
    let node = &plan.nodes[index];
    if node.other_page {
        return Err(Error::unsupported(
            super::REPLACEMENT,
            "text.structure-replacement: structure replacement spans multiple pages",
        ));
    }
    let mut selected = Vec::new();
    for item in &plan.items {
        budget.work(1)?;
        if item.node < index || item.node >= node.end || plan.nodes[item.node].artifact {
            continue;
        }
        for range in &item.ranges {
            budget.work(range.len())?;
            for glyph in range.clone() {
                let g = &page.glyphs()[glyph];
                if !g.hidden
                    && !g
                        .marked_content
                        .iter()
                        .any(|m| m.tag.as_bytes() == b"Artifact")
                {
                    budget.store(size_of::<usize>())?;
                    selected.push(glyph);
                }
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    if selected.windows(2).any(|w| w[1] != w[0] + 1) {
        return Err(Error::unsupported(
            super::REPLACEMENT,
            "text.structure-replacement: structure replacement needs noncontiguous glyph associations",
        ));
    }
    let glyphs = selected
        .first()
        .zip(selected.last())
        .map_or(0..0, |(&a, &b)| a..b + 1);
    let chars: Vec<_> = node
        .replacement
        .iter()
        .flat_map(|s| s.text.chars())
        .map(Some)
        .collect();
    budget.store(chars.capacity().saturating_mul(size_of::<Option<char>>()))?;
    Ok(Atom {
        chars,
        geometry: selected.first().copied(),
        glyphs,
        marked: None,
        occurrence: None,
        replacement: node.replacement.as_ref().map(|r| r.kind),
    })
}
fn block(plan: &Plan<'_>, index: usize) -> Option<(usize, bool)> {
    let mut current = Some(index);
    while let Some(i) = current {
        let node = &plan.nodes[i];
        if let Some(role) = &node.element.standard_type {
            let paragraph = super::roles::heading(role.as_bytes())
                || matches!(
                    role.as_bytes(),
                    b"P" | b"H" | b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6" | b"Title"
                );
            if paragraph || matches!(role.as_bytes(), b"LI" | b"LBody" | b"TD" | b"TH") {
                return Some((i, paragraph));
            }
        }
        current = node.element.parent;
    }
    None
}
struct Output<'a> {
    page: &'a TextPage,
    logical: LogicalText,
    last: Option<usize>,
    block: Option<(usize, bool)>,
    replacement: Option<Replacement>,
}
impl Output<'_> {
    fn emit(
        &mut self,
        atom: &Atom,
        block: Option<(usize, bool)>,
        budget: &mut Budget,
    ) -> Result<()> {
        let actual_pair = self.replacement == Some(Replacement::Actual)
            && atom.replacement == Some(Replacement::Actual);
        if !self.logical.text.is_empty() && !atom.chars.is_empty() && !actual_pair {
            if self.block != block && (self.block.is_some() || block.is_some()) {
                self.separator('\n', budget)?;
                if self.block.is_some_and(|b| b.1) || block.is_some_and(|b| b.1) {
                    self.separator('\n', budget)?;
                }
            } else if self.replacement == Some(Replacement::Alt)
                || atom.replacement == Some(Replacement::Alt)
            {
                if !self.logical.text.ends_with(char::is_whitespace)
                    && !atom
                        .chars
                        .first()
                        .is_some_and(|c| c.is_some_and(char::is_whitespace))
                {
                    self.separator(' ', budget)?;
                }
            } else if let Some((a, b)) = self.last.zip(atom.geometry) {
                let (a, b) = (&self.page.glyphs()[a], &self.page.glyphs()[b]);
                if !same_line(a, b, crate::LayoutOptions::default()) {
                    self.separator('\n', budget)?;
                } else if gap(a, b)
                    > crate::LayoutOptions::default().space_gap * a.em.max(b.em).max(1e-6)
                    && !self.logical.text.ends_with(char::is_whitespace)
                    && !atom
                        .chars
                        .first()
                        .is_some_and(|c| c.is_some_and(char::is_whitespace))
                {
                    self.separator(' ', budget)?;
                }
            }
        }
        for &c in &atom.chars {
            self.check(budget, c.unwrap_or('\u{fffd}').len_utf8())?;
            append(
                &mut self.logical.characters,
                &mut self.logical.text,
                c,
                atom.glyphs.clone(),
                false,
            );
        }
        self.last = atom
            .glyphs
            .end
            .checked_sub(1)
            .filter(|_| !atom.glyphs.is_empty())
            .or(atom.geometry);
        self.block = block;
        self.replacement = atom.replacement;
        Ok(())
    }
    fn check(&self, budget: &mut Budget, bytes: usize) -> Result<()> {
        if self.logical.text.len().saturating_add(bytes) > self.page.limits.string_bytes {
            return Err(Error::limit("logical text string budget exceeded"));
        }
        budget.work(1)?;
        budget.store(size_of::<crate::page::Character>() + bytes * 2)
    }
    fn separator(&mut self, c: char, budget: &mut Budget) -> Result<()> {
        self.check(budget, c.len_utf8())?;
        append(
            &mut self.logical.characters,
            &mut self.logical.text,
            Some(c),
            0..0,
            true,
        );
        Ok(())
    }
}
