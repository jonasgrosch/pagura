//! Inert navigation views (ISO 32000-2 clauses 7.9 and 12.3–12.6).
//!
//! These APIs never open a file, follow a URI or execute an action. Results
//! borrow immutable source data. Flat outline/action lists avoid recursive
//! ownership. Each operation uses `Limits::depth` and the smaller of
//! `Limits::value_nodes` and `Limits::objects` for traversal, and
//! `Limits::document_bytes` for estimated temporary/result storage. Results
//! are caller-owned and uncached; source objects use the usual retained budget.
pub(crate) mod actions;
mod destinations;
mod labels;
mod links;
mod outlines;
mod trees;

pub use actions::{Action, ActionKind, ActionSequence, NamedAction, Script};
pub use destinations::{
    Destination, DestinationName, DestinationView, NamedDestination, RemoteDestination, Target,
};

pub use labels::{LabelStyle, PageLabelRange, PageLabels};
pub use links::{BorderStyle, HighlightMode, Link, LinkBorder};
pub use outlines::OutlineItem;
pub use trees::{NameTree, NumberTree, Tree, TreeEntries, TreeKey};

use crate::{Document, Page};
use pagura_core::{Error, Result, Value};

impl Document {
    /// A borrowed name-tree handle (7.9.6), including non-navigation trees.
    pub fn name_tree<'a>(&'a self, root: &'a Value) -> NameTree<'a> {
        Tree::new(self, root)
    }

    /// A borrowed number-tree handle (7.9.7).
    pub fn number_tree<'a>(&'a self, root: &'a Value) -> NumberTree<'a> {
        Tree::new(self, root)
    }

    /// Resolves an explicit or named local destination (12.3.2).
    ///
    /// # Errors
    /// Returns positioned syntax, missing-target, cycle or budget errors.
    pub fn destination(&self, value: &Value) -> Result<Destination> {
        destinations::local(self, value)
    }

    /// Lists both catalog destination collections, preserving their key kinds.
    ///
    /// # Errors
    /// Returns errors from tree validation or destination resolution.
    pub fn named_destinations(&self) -> Result<Vec<NamedDestination<'_>>> {
        destinations::named(self)
    }

    /// Outline items in preorder; `parent` indexes this flat list (12.3.3).
    ///
    /// # Errors
    /// Returns positioned structural, target, text or budget errors. Safe link
    /// and count corrections are reported repairs under ADR 0003.
    pub fn outlines(&self) -> Result<Vec<OutlineItem<'_>>> {
        outlines::read(self)
    }

    /// Parses an inert action and its Next tree in preorder (12.6).
    ///
    /// # Errors
    /// Returns positioned syntax, cycle or budget errors.
    pub fn actions<'a>(&'a self, value: &'a Value) -> Result<ActionSequence<'a>> {
        actions::read(self, value)
    }

    /// Label ranges from the catalog number tree (12.4.2, Table 161).
    ///
    /// # Errors
    /// Returns positioned malformed-label/tree or budget errors.
    pub fn page_label_ranges(&self) -> Result<Vec<PageLabelRange>> {
        labels::ranges(self)
    }

    /// Parses label ranges once for indexed lookup or linear page-order
    /// iteration (12.4.2). Prefer this view when displaying many labels.
    ///
    /// # Errors
    /// As [`Document::page_label_ranges`] and [`Document::pages`].
    pub fn page_labels(&self) -> Result<PageLabels> {
        labels::read(self)
    }

    /// The display label of a zero-based page; absent PageLabels uses decimal.
    ///
    /// # Errors
    /// Returns an invalid-argument error for an out-of-range index, or errors
    /// interpreting labels and enforcing string/result budgets.
    pub fn page_label(&self, index: usize) -> Result<String> {
        if index >= self.page_count()? {
            return Err(Error::invalid_argument("page label index outside document"));
        }
        self.page_labels()?.label(index)
    }
}

impl Page {
    /// This page's Link annotations as inert targets (12.5.6.5, Table 176).
    ///
    /// # Errors
    /// Returns positioned annotation, action, destination or budget errors,
    /// and `InvalidArgument` if `document` is not the one this page came from.
    pub fn links<'a>(&'a self, document: &'a Document) -> Result<Vec<Link<'a>>> {
        links::read(document, self)
    }

    /// This page's display label (12.4.2).
    ///
    /// # Errors
    /// As [`Document::page_label`], and `InvalidArgument` if `document` is not
    /// the one this page came from.
    pub fn label(&self, document: &Document) -> Result<String> {
        document.check_page(self)?;
        document.page_label(self.index())
    }
}

#[cfg(test)]
mod tests;
