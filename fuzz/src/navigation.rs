//! `navigation`: name and number trees, destinations, named destinations,
//! outlines, link annotations, actions and page labels (clauses 7.9.6–7.9.7,
//! 12.3–12.6) are bounded, inert views or structured errors in both recovery
//! modes.
//!
//! The input is a PDF file. Every view stays within one traversal budget of
//! items, resolves local destinations to pages inside the document, lists
//! parents before their children, and formats one label per page. Actions
//! are data: each one is read back from its own dictionary, and reading a
//! view again returns the same result, so evaluating it ran nothing.

use pagura::navigation::{
    Action, ActionKind, BorderStyle, Destination, DestinationName, DestinationView, Link,
    NamedAction, OutlineItem, PageLabels, Script, Target,
};
use pagura::{Dictionary, Document, ErrorKind, Name, OpenOptions, Page, Recovery, Rect, Value};

/// How many pages have their links and labels read.
const PAGES: usize = 16;
/// How many destination names and tree keys are looked up one by one.
const LOOKUPS: usize = 64;

/// Opens the input as a PDF file and reads every navigation view.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let options = OpenOptions {
            limits: super::limits(),
            recovery,
            ..OpenOptions::default()
        };
        let Some(document) = super::checked(Document::open(data.to_vec(), &options)) else {
            continue;
        };
        let views = Views {
            document: &document,
            pages: document.page_count().unwrap_or(0),
            budget: document.limits().value_nodes.min(document.limits().objects),
        };
        views.outlines();
        views.named_destinations();
        views.trees();
        views.open_action();
        let labels = views.labels();
        if let Some(pages) = super::checked(document.pages()) {
            for page in pages.iter().take(PAGES) {
                views.links(page);
                views.label(page, labels.as_ref());
            }
        }
        super::assert_diagnostics(&document.diagnostics(), recovery);
    }
}

/// The document under test, its page count (zero without a page tree, so
/// that no destination can be in range) and the traversal budget of one view.
struct Views<'a> {
    document: &'a Document,
    pages: usize,
    budget: usize,
}

impl Views<'_> {
    /// Outline items in preorder, each after its parent and one level deeper.
    fn outlines(&self) {
        let first = self.document.outlines();
        assert!(
            first == self.document.outlines(),
            "outlines differ between two reads"
        );
        let Some(items) = super::checked(first) else {
            return;
        };
        assert!(items.len() <= self.budget, "{} outline items", items.len());
        let limits = self.document.limits();
        for (index, item) in items.iter().enumerate() {
            assert_preorder(&items, index);
            assert!(item.depth <= limits.depth, "outline depth {}", item.depth);
            assert!(
                item.title.len() <= limits.string_bytes,
                "outline title of {} bytes",
                item.title.len()
            );
            assert!(
                item.color.iter().all(|c| (0.0..=1.0).contains(c)),
                "outline colour {:?}",
                item.color
            );
            self.target(item.target.as_ref());
        }
    }

    /// Every listed name resolves again, one by one, to the same page.
    fn named_destinations(&self) {
        let first = self.document.named_destinations();
        assert!(
            first == self.document.named_destinations(),
            "named destinations differ between two reads"
        );
        let Some(named) = super::checked(first) else {
            return;
        };
        assert!(
            named.len() <= self.budget,
            "{} named destinations",
            named.len()
        );
        for entry in &named {
            self.destination(&entry.destination);
        }
        for entry in named.iter().take(LOOKUPS) {
            match entry.name {
                // A legacy name is looked up where it was listed, within a
                // fresh traversal budget.
                DestinationName::Name(name) => {
                    let found = self.document.destination(&Value::Name(name.clone()));
                    assert_eq!(found, Ok(entry.destination), "{name} resolves differently");
                }
                // A name-tree key may have duplicates the lookup is free to
                // choose among; that it is found at all is checked in `trees`.
                DestinationName::String(bytes) => self.lookup(&Value::String(bytes.to_vec())),
            }
        }
    }

    /// The catalog's name trees, the legacy destination dictionary and the
    /// page-label number tree: every listed key is found again, destination
    /// names resolve or fail cleanly, and document-level scripts are inert.
    fn trees(&self) {
        let Some(catalog) = super::checked(self.document.catalog()) else {
            return;
        };
        if let Some(names) = self.dictionary(catalog, b"Names") {
            for (category, root) in names.iter() {
                let tree = self.document.name_tree(root);
                let Some(entries) = super::checked(tree.entries()) else {
                    continue;
                };
                assert!(
                    entries.len() <= self.budget,
                    "{} tree entries",
                    entries.len()
                );
                for (key, value) in entries.take(LOOKUPS) {
                    let found = super::checked(tree.get(key));
                    assert!(
                        found != Some(None),
                        "listed name-tree key {key:?} is absent"
                    );
                    match category.as_bytes() {
                        b"Dests" => self.lookup(&Value::String(key.to_vec())),
                        b"JavaScript" => self.actions(value),
                        _ => {}
                    }
                }
            }
        }
        if let Some(dests) = self.dictionary(catalog, b"Dests") {
            for (name, _) in dests.iter().take(LOOKUPS) {
                self.lookup(&Value::Name(name.clone()));
            }
        }
        if let Some(root) = catalog.get(b"PageLabels") {
            let tree = self.document.number_tree(root);
            if let Some(entries) = super::checked(tree.entries()) {
                assert!(
                    entries.len() <= self.budget,
                    "{} tree entries",
                    entries.len()
                );
                for (key, _) in entries.take(LOOKUPS) {
                    let found = super::checked(tree.get(&key));
                    assert!(
                        found != Some(None),
                        "listed number-tree key {key} is absent"
                    );
                }
            }
        }
    }

    /// The catalog's OpenAction, a destination array or an action.
    fn open_action(&self) {
        let Some(catalog) = super::checked(self.document.catalog()) else {
            return;
        };
        let Some(value) = catalog.get(b"OpenAction") else {
            return;
        };
        match super::checked(self.document.resolve(value)) {
            Some(Value::Array(_)) => self.lookup(value),
            Some(Value::Dictionary(_)) => self.actions(value),
            _ => {}
        }
    }

    /// Ranges ascend from page zero, and formatting all labels in page order
    /// agrees with formatting each one on its own.
    fn labels(&self) -> Option<PageLabels> {
        let ranges = self.document.page_label_ranges();
        let labels = super::checked(self.document.page_labels())?;
        assert!(
            ranges.as_deref() == Ok(labels.ranges()),
            "page label ranges differ between two reads"
        );
        assert!(
            labels.ranges().first().is_none_or(|r| r.page == 0),
            "the first label range starts after page zero"
        );
        for pair in labels.ranges().windows(2) {
            assert!(pair[0].page < pair[1].page, "label ranges do not ascend");
        }
        for range in labels.ranges() {
            assert!(
                range.page < self.pages,
                "label range at page {}",
                range.page
            );
            assert!(range.start >= 1, "label range starting at {}", range.start);
        }
        let maximum = {
            let limits = self.document.limits();
            limits.string_bytes.min(limits.document_bytes)
        };
        let all = labels.iter();
        assert_eq!(all.len(), self.pages, "labels for {} pages", self.pages);
        for (index, label) in all.enumerate() {
            if let Ok(text) = &label {
                assert!(text.len() <= maximum, "page label of {} bytes", text.len());
            }
            assert!(label == labels.label(index), "label {index} differs");
            super::checked(label);
        }
        let past = labels.label(self.pages).map_err(|e| e.kind);
        assert_eq!(
            past,
            Err(ErrorKind::InvalidArgument),
            "a label past the last page"
        );
        Some(labels)
    }

    /// A page's own label, which reads the labels again, agrees with them.
    fn label(&self, page: &Page, labels: Option<&PageLabels>) {
        let label = page.label(self.document);
        if let Some(labels) = labels {
            assert!(
                label == labels.label(page.index()),
                "page {} label differs between two reads",
                page.index()
            );
        }
        super::checked(label);
    }

    /// Link rectangles and quadrilaterals, borders and targets.
    fn links(&self, page: &Page) {
        let first = page.links(self.document);
        assert!(
            first == page.links(self.document),
            "links differ between two reads"
        );
        let Some(links) = super::checked(first) else {
            return;
        };
        assert!(links.len() <= self.budget, "{} links", links.len());
        for link in &links {
            assert_link(self.document, link);
            self.target(link.target.as_ref());
        }
    }

    /// A destination looked up by value: a page in range or a structured
    /// error.
    fn lookup(&self, value: &Value) {
        if let Some(destination) = super::checked(self.document.destination(value)) {
            self.destination(&destination);
        }
    }

    /// An action tree read on its own.
    fn actions(&self, value: &Value) {
        let first = self.document.actions(value);
        assert!(
            first == self.document.actions(value),
            "actions differ between two reads"
        );
        if let Some(sequence) = super::checked(first) {
            self.action_list(&sequence.actions);
        }
    }

    fn target(&self, target: Option<&Target<'_>>) {
        match target {
            None => {}
            Some(Target::Destination(destination)) => self.destination(destination),
            Some(Target::Actions(sequence)) => self.action_list(&sequence.actions),
        }
    }

    /// Flat preorder actions: the initial one has no parent, every other one
    /// follows its parent, and each is the data of its own dictionary.
    fn action_list(&self, actions: &[Action<'_>]) {
        assert!(!actions.is_empty(), "an empty action sequence");
        assert!(actions.len() <= self.budget, "{} actions", actions.len());
        for (index, action) in actions.iter().enumerate() {
            match action.parent {
                None => assert_eq!(index, 0, "action {index} has no parent"),
                Some(parent) => assert!(parent < index, "action {index} follows {parent}"),
            }
            self.action(action);
        }
    }

    fn action(&self, action: &Action<'_>) {
        let entry = |key: &[u8]| {
            action
                .dictionary
                .get(key)
                .and_then(|v| self.document.resolve(v).ok())
        };
        let subtype = entry(b"S").and_then(Value::as_name).map(Name::as_bytes);
        let expected: &[u8] = match &action.kind {
            ActionKind::GoTo(destination) => {
                self.destination(destination);
                b"GoTo"
            }
            ActionKind::GoToR { file, .. } => {
                assert!(
                    matches!(file, Value::String(_) | Value::Dictionary(_)),
                    "GoToR file {file:?}"
                );
                b"GoToR"
            }
            ActionKind::Uri { uri, .. } => {
                assert_eq!(entry(b"URI").and_then(Value::as_string), Some(*uri), "URI");
                b"URI"
            }
            ActionKind::Named(named) => {
                let name = entry(b"N").and_then(Value::as_name).map(Name::as_bytes);
                let standard: Option<&[u8]> = match named {
                    NamedAction::NextPage => Some(b"NextPage"),
                    NamedAction::PrevPage => Some(b"PrevPage"),
                    NamedAction::FirstPage => Some(b"FirstPage"),
                    NamedAction::LastPage => Some(b"LastPage"),
                    NamedAction::Other(other) => {
                        assert_eq!(name, Some(other.as_bytes()), "named action");
                        None
                    }
                };
                if standard.is_some() {
                    assert_eq!(name, standard, "named action");
                }
                b"Named"
            }
            ActionKind::Launch { file, .. } => {
                assert!(
                    file.is_none_or(|f| matches!(f, Value::String(_) | Value::Dictionary(_))),
                    "Launch file {file:?}"
                );
                b"Launch"
            }
            ActionKind::JavaScript(script) => {
                match script {
                    Script::String(bytes) => {
                        assert_eq!(entry(b"JS").and_then(Value::as_string), Some(*bytes));
                    }
                    Script::Stream(object) => assert!(object.is_stream(), "script object"),
                }
                b"JavaScript"
            }
            ActionKind::Other(name) => {
                assert!(
                    ![
                        &b"GoTo"[..],
                        b"GoToR",
                        b"URI",
                        b"Named",
                        b"Launch",
                        b"JavaScript",
                    ]
                    .contains(&name.as_bytes()),
                    "a standard action listed as {name}"
                );
                name.as_bytes()
            }
        };
        assert_eq!(subtype, Some(expected), "action of kind {:?}", action.kind);
    }

    /// A local destination: a page of this document and a finite view.
    fn destination(&self, destination: &Destination) {
        assert!(
            destination.page < self.pages,
            "destination page {} of {}",
            destination.page,
            self.pages
        );
        let finite = |values: &[Option<f64>]| values.iter().flatten().all(|v| v.is_finite());
        let view = destination.view;
        let valid = match view {
            DestinationView::Xyz { left, top, zoom } => {
                finite(&[left, top, zoom]) && zoom.is_none_or(|z| z > 0.0)
            }
            DestinationView::Fit | DestinationView::FitB => true,
            DestinationView::FitH(value)
            | DestinationView::FitV(value)
            | DestinationView::FitBh(value)
            | DestinationView::FitBv(value) => finite(&[value]),
            DestinationView::FitR(rect) => normalized(rect),
        };
        assert!(valid, "destination view {view:?}");
    }

    /// A catalog entry that resolves to a dictionary.
    fn dictionary<'a>(&'a self, catalog: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
        catalog
            .get(key)
            .and_then(|value| super::checked(self.document.resolve_dictionary(value)))
    }
}

/// Item `index` follows its parent in preorder: the parent is the previous
/// item or one of its ancestors, and the item is one level deeper.
fn assert_preorder(items: &[OutlineItem<'_>], index: usize) {
    let item = &items[index];
    let Some(parent) = item.parent else {
        assert_eq!(item.depth, 0, "top-level outline item {index} is nested");
        return;
    };
    assert!(parent < index, "outline item {index} precedes its parent");
    assert_eq!(
        item.depth,
        items[parent].depth + 1,
        "outline item {index} depth"
    );
    let mut ancestor = Some(index - 1);
    while let Some(at) = ancestor.filter(|&at| at != parent) {
        ancestor = items[at].parent;
    }
    assert!(
        ancestor.is_some(),
        "outline item {index} is not in preorder"
    );
}

/// A link's geometry is normalized and finite, quadrilaterals lie inside the
/// rectangle (Table 176), and the border is a nonnegative pattern.
fn assert_link(document: &Document, link: &Link<'_>) {
    assert!(normalized(link.rectangle), "link {:?}", link.rectangle);
    let r = link.rectangle;
    for quad in &link.quad_points {
        for xy in quad.as_chunks::<2>().0 {
            assert!(
                (r.x0()..=r.x1()).contains(&xy[0]) && (r.y0()..=r.y1()).contains(&xy[1]),
                "quad point {xy:?} outside {r:?}"
            );
        }
    }
    let border = &link.border;
    assert!(
        [
            border.horizontal_radius,
            border.vertical_radius,
            border.width
        ]
        .iter()
        .chain(&border.dash)
        .all(|v| v.is_finite() && *v >= 0.0),
        "link border {border:?}"
    );
    assert!(
        border.dash.is_empty() || border.style == BorderStyle::Dashed,
        "a dash pattern on a {:?} border",
        border.style
    );
    let subtype = link
        .dictionary
        .get(b"Subtype")
        .and_then(|v| document.resolve(v).ok())
        .and_then(Value::as_name)
        .map(Name::as_bytes);
    assert_eq!(subtype, Some(&b"Link"[..]), "a link of another subtype");
}

fn normalized(rect: Rect) -> bool {
    [rect.x0(), rect.y0(), rect.x1(), rect.y1()]
        .iter()
        .all(|v| v.is_finite())
        && rect.x0() <= rect.x1()
        && rect.y0() <= rect.y1()
}
