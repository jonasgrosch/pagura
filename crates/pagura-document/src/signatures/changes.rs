//! Structural policy results, conditional on separate cryptographic verification.
use pagura_core::ObjectId;

/// Structural modification analysis (12.8.2). No digest or signer is verified.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermittedModifications {
    /// No usable signed revision has been established.
    NotAnalysedYet,
    /// Revision history or policy cannot be interpreted safely.
    Inconclusive,
    /// Object comparisons under the policy frozen at the signed revision.
    Analysed(ModificationAnalysis),
}
/// Overall structural result. `Allowed` is conditional on authenticating the
/// signed bytes and transform parameters; it is never signature validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModificationOutcome {
    /// No semantic object or trailer changes.
    NoChanges,
    /// Every change is permitted by the applicable supported policies.
    Allowed,
    /// At least one change violates a supported policy.
    Disallowed,
    /// At least one change cannot be classified, with no proven violation.
    Inconclusive,
}
/// One independent policy decision for a changed object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeDecision {
    /// Permitted by the policy.
    Allowed,
    /// Forbidden by the policy.
    Disallowed,
    /// The role or policy cannot be established.
    Inconclusive,
    /// No such transform restricts this signature.
    NotApplicable,
}
/// Object identity or the effective trailer (which has no object identity);
/// nothing else can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeTarget {
    /// A generation-sensitive indirect identity.
    Object(ObjectId),
    /// Effective trailer entries, excluding xref bookkeeping.
    Trailer,
}
/// Semantic difference; a byte-only rewrite with equal values and stream bytes
/// is omitted. Stream encoding changes are retained, even if they render alike.
/// An identity can only be added, removed or modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Identity became in use.
    Added,
    /// Identity became free or absent.
    Removed,
    /// Value or encoded stream bytes changed.
    Modified,
}
/// A use established by reachability, never merely an object's `/Type` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ObjectRole {
    /// Field dictionary or its structural configuration.
    Field,
    /// A field's current value, including indirect text values.
    FieldValue,
    /// Widget dictionary.
    Widget,
    /// Widget appearance and referenced rendering resources.
    WidgetAppearance,
    /// Non-widget annotation on an actual page.
    Annotation,
    /// Annotation appearance and referenced rendering resources.
    AnnotationAppearance,
    /// Page contents, resources or other rendering dependencies.
    PageContent,
    /// Page dictionary.
    Page,
    /// Page tree node.
    PageTree,
    /// Document catalog.
    Catalog,
    /// Document information dictionary and its values (14.3.3).
    DocumentInformation,
    /// Interactive form container.
    AcroForm,
    /// Catalog permission dictionary.
    Permissions,
    /// Signature value attached to an actual signature field.
    Signature,
    /// Cross-reference stream at a confirmed xref location, or an object-stream
    /// container whose member objects are compared separately (7.5.7).
    CrossReference,
    /// Unrecognized or conflicting use; never grants permission.
    Other,
}
/// Clause-based rule used to explain a structural decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModificationRule {
    /// No ordinary changes at DocMDP P=1 (12.8.2.2, Table 257).
    NoChanges,
    /// Current form value and associated appearance (12.8.2.2, Table 257).
    FormFill,
    /// Addition of a signature (12.8.2.2, Table 257).
    Signing,
    /// Annotation creation, deletion or modification at P=3 (Table 257).
    Annotation,
    /// Page content or document structure is outside the permitted operations.
    DocumentStructure,
    /// Field selection frozen at signing (12.8.2.4, Table 259; Table 236).
    FieldLock,
    /// Xref bookkeeping (7.5.6) or the changing file identifier (14.4),
    /// with other trailer semantics checked separately.
    CrossReference,
    /// Unsupported role, transformation or exception; no permission inferred.
    Unclassified,
}
/// Complete object-level evidence for a change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectChange {
    /// Changed identity.
    pub target: ChangeTarget,
    /// Kind of change.
    pub kind: ChangeKind,
    /// Every established role across both compared revisions.
    pub roles: Vec<ObjectRole>,
    /// DocMDP decision, separately from field restrictions.
    pub doc_mdp: ChangeDecision,
    /// FieldMDP and signed field-lock decision.
    pub field_mdp: ChangeDecision,
    /// Explanation for the DocMDP classification.
    pub rule: ModificationRule,
    /// An annotation or widget may cover signed page content (12.5.5).
    /// Includes signing widgets and added appearances. This conservative
    /// warning does not change the permitted-change decision.
    pub visible_overlay: bool,
}
/// One later incremental update. Comparisons include both net differences
/// against the signed revision and differences against the preceding update.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RevisionChanges {
    /// Index in `Document::revisions()`, newest first.
    pub revision: usize,
    /// Differences from the immediately preceding source revision.
    pub changes: Vec<ObjectChange>,
    /// Differences from the signed revision.
    pub net_changes: Vec<ObjectChange>,
    /// Cumulative result through this update, retaining reverted violations.
    pub outcome: ModificationOutcome,
}
/// Bounded structural comparisons, in chronological update order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModificationAnalysis {
    /// Index of the signed revision in the original document.
    pub signed_revision: usize,
    /// Each later incremental update, oldest first.
    pub revisions: Vec<RevisionChanges>,
    /// Cumulative outcome, including changes later reverted.
    pub outcome: ModificationOutcome,
}
