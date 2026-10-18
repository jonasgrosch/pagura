//! Typed source views for interactive forms (12.7).
use crate::navigation::ActionSequence;
use pagura_core::{Dictionary, Name, ObjectId, Position, Rect, Value};

/// All field flags, preserving extension/reserved bits (Table 227).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldFlags(pub u32);
impl FieldFlags {
    /// Whether a one-based bit position (1..=32) is set.
    pub const fn contains(self, bit: u32) -> bool {
        bit > 0 && bit <= 32 && self.0 & (1 << (bit - 1)) != 0
    }
    /// The field cannot be changed interactively.
    pub const fn read_only(self) -> bool {
        self.contains(1)
    }
    /// A value is required when exporting.
    pub const fn required(self) -> bool {
        self.contains(2)
    }
    /// The field is excluded from export.
    pub const fn no_export(self) -> bool {
        self.contains(3)
    }
}
/// Button interaction kind (12.7.5.2, Table 229).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// A momentary control, without a permanent value.
    Push,
    /// A check box.
    CheckBox,
    /// A group of radio widgets; flags bit 26 controls shared on-states.
    Radio,
}
/// Quadding for variable text (Table 228).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Justification {
    /// Left (the default).
    #[default]
    Left,
    /// Centre.
    Centre,
    /// Right.
    Right,
}
/// One choice option, retained in source order (Table 234).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// Export text; equal to display for the single-string form.
    pub export: String,
    /// Display text.
    pub display: String,
}
/// Typed terminal field configuration; nonterminal nodes have no logical type.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind<'a> {
    /// An attribute/name container with child fields.
    Group,
    /// Push button, check box or radio group.
    Button {
        /// Interaction kind.
        kind: ButtonKind,
        /// Optional export strings, in widget order (Table 230).
        export_values: Vec<String>,
    },
    /// Text configuration (12.7.5.3).
    Text {
        /// Maximum number of characters, when specified.
        max_length: Option<usize>,
        /// More than one line is permitted.
        multiline: bool,
        /// Password input (the API does not hide existing source values).
        password: bool,
        /// Fixed character cells.
        comb: bool,
        /// RichText flag; rich text is never evaluated.
        rich_text: bool,
        /// Original RV data, a string or stream reference, without XML parsing.
        rich_value: Option<&'a Value>,
    },
    /// Choice configuration (12.7.5.4).
    Choice {
        /// Combo box when true, list box otherwise.
        combo: bool,
        /// An editable combo box.
        editable: bool,
        /// Multiple selections are allowed.
        multi_select: bool,
        /// Options in source order, never resorted by the reader.
        options: Vec<ChoiceOption>,
    },
    /// Signature presence only; cryptographic validity is not assessed.
    Signature {
        /// A non-null V is present (7.3.9, 12.7.5.5).
        signed: bool,
    },
    /// Extension field type, preserved without inferred semantics.
    Other(&'a Name),
}
/// A field value (12.7.5), distinct from unmodified source entries.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue<'a> {
    /// Decoded text string or text stream.
    Text(String),
    /// Button appearance-state name, including Off.
    Name(&'a Name),
    /// Multiple selected choice strings.
    Texts(Vec<String>),
    /// Signature dictionary, without signature validation.
    Signature(&'a Dictionary),
    /// Extension field data.
    Other(&'a Value),
}
/// An additional-action entry (12.6.3, Tables 196–197), never executed.
#[derive(Debug, Clone, PartialEq)]
pub struct AdditionalAction<'a> {
    /// Trigger name, preserving extension events too.
    pub trigger: &'a Name,
    /// Typed inert action/Next sequence.
    pub actions: ActionSequence<'a>,
}
/// A field node and its effective inherited attributes (12.7.4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Field<'a> {
    /// Source object, or enclosing object for a repaired direct dictionary.
    pub object: Option<ObjectId>,
    /// Source position, including compressed-object context.
    pub position: Position,
    /// Parent field index, or None for a root.
    pub parent: Option<usize>,
    /// Decoded partial name; None means an anonymous repaired field.
    pub partial_name: Option<String>,
    /// Ancestor names joined with periods. Repairs preserve malformed names
    /// verbatim; this is not a guaranteed unique identifier.
    pub qualified_name: String,
    /// Alternative accessible name TU, without inheritance.
    pub alternative_name: Option<String>,
    /// Export mapping name TM, without inheritance.
    pub mapping_name: Option<String>,
    /// Effective flags.
    pub flags: FieldFlags,
    /// Terminal field configuration, or Group for a container.
    pub kind: FieldKind<'a>,
    /// Effective V, or None for absent/null.
    pub value: Option<FieldValue<'a>>,
    /// Effective DV, or None for absent/null.
    pub default_value: Option<FieldValue<'a>>,
    /// Effective default appearance bytes DA, including AcroForm fallback.
    pub default_appearance: Option<&'a [u8]>,
    /// Effective Q, including AcroForm fallback.
    pub justification: Justification,
    /// Own additional actions (not inherited), including merged widget AA.
    pub additional_actions: Vec<AdditionalAction<'a>>,
    /// Unmodified own dictionary, without inherited entries inserted.
    pub dictionary: &'a Dictionary,
}
/// A widget associated with a terminal field (12.5.6.19, 12.7.4.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Widget<'a> {
    /// Index of the owning field.
    pub field: usize,
    /// Source object or enclosing object for a direct widget.
    pub object: Option<ObjectId>,
    /// Source position.
    pub position: Position,
    /// Whether widget and field share their dictionary.
    pub merged: bool,
    /// Annotation rectangle, normalized to increasing coordinates.
    pub rectangle: Rect,
    /// Non-Off keys of AP/N, in dictionary order. Repeated names across
    /// radio widgets are retained; they do not identify a unique widget.
    pub on_states: Vec<&'a Name>,
    /// Current AS state, if supplied.
    pub appearance_state: Option<&'a Name>,
    /// Primary activation action A, inert.
    pub action: Option<ActionSequence<'a>>,
    /// Additional actions of a separate widget. For a merged dictionary, use
    /// the owning field's additional_actions; the sequence is parsed once.
    pub additional_actions: Vec<AdditionalAction<'a>>,
    /// Unmodified annotation dictionary.
    pub dictionary: &'a Dictionary,
}
/// The document's interactive form, as immutable source views (Table 224).
#[derive(Debug, Clone, PartialEq)]
pub struct AcroForm<'a> {
    /// Fields in preorder; parent indexes precede children.
    pub fields: Vec<Field<'a>>,
    /// Widgets in field/source order, associated through Widget::field.
    pub widgets: Vec<Widget<'a>>,
    /// Deprecated appearance-generation request, retained for explicit preparation.
    pub need_appearances: bool,
    /// Signature flags (Table 225), without signature validation.
    pub signature_flags: u32,
    /// Calculation-order entries, kept as data and never executed.
    pub calculation_order: &'a [Value],
    /// Default resources, borrowed once rather than copied per field.
    pub default_resources: Option<&'a Dictionary>,
    /// Document-wide DA bytes, retained without changing source data.
    pub default_appearance: Option<&'a [u8]>,
    /// Document-wide Q, defaulting to Left.
    pub justification: Justification,
    /// Non-null XFA exists; it is deprecated and is not interpreted.
    pub has_xfa: bool,
    /// Unmodified AcroForm dictionary, retaining extension data.
    pub dictionary: &'a Dictionary,
}
