//! Typed source views for interactive forms (12.7).
use crate::navigation::ActionSequence;
use pagura_core::{Dictionary, Name, ObjectId, Position, Rect, Value};

/// All field flags, preserving extension and reserved bits (Table 227).
///
/// The constants name the bits of Tables 227, 229, 231 and 233. Bits 23 and
/// 26 mean different things for different field types, so two constants
/// share each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldFlags(u32);

impl FieldFlags {
    /// The field cannot be changed interactively (all types, bit 1).
    pub const READ_ONLY: Self = Self::bit(1);
    /// A value is required when exporting (all types, bit 2).
    pub const REQUIRED: Self = Self::bit(2);
    /// The field is excluded from export (all types, bit 3).
    pub const NO_EXPORT: Self = Self::bit(3);
    /// A text field may hold several lines (bit 13).
    pub const MULTILINE: Self = Self::bit(13);
    /// A text field is a password entry (bit 14).
    pub const PASSWORD: Self = Self::bit(14);
    /// Clicking the selected radio button does not turn it off (bit 15).
    pub const NO_TOGGLE_TO_OFF: Self = Self::bit(15);
    /// A button field is a set of radio buttons (bit 16).
    pub const RADIO: Self = Self::bit(16);
    /// A button field is a push button (bit 17).
    pub const PUSHBUTTON: Self = Self::bit(17);
    /// A choice field is a combo box rather than a list box (bit 18).
    pub const COMBO: Self = Self::bit(18);
    /// A combo box also accepts typed text (bit 19).
    pub const EDIT: Self = Self::bit(19);
    /// Choice options should be shown sorted (bit 20).
    pub const SORT: Self = Self::bit(20);
    /// A text field names a file to submit (bit 21).
    pub const FILE_SELECT: Self = Self::bit(21);
    /// A choice field allows several selections (bit 22).
    pub const MULTI_SELECT: Self = Self::bit(22);
    /// Text is not spell-checked (text and choice fields, bit 23).
    pub const DO_NOT_SPELL_CHECK: Self = Self::bit(23);
    /// A text field does not scroll beyond its area (bit 24).
    pub const DO_NOT_SCROLL: Self = Self::bit(24);
    /// A text field is split into MaxLen equal cells (bit 25).
    pub const COMB: Self = Self::bit(25);
    /// A text field holds rich text (text fields, bit 26).
    pub const RICH_TEXT: Self = Self::bit(26);
    /// Radio buttons with the same on state turn on together (buttons, bit 26).
    pub const RADIOS_IN_UNISON: Self = Self::bit(26);
    /// A choice value is committed on every selection change (bit 27).
    pub const COMMIT_ON_SEL_CHANGE: Self = Self::bit(27);

    /// The flag at one-based bit `position`, as the tables number them.
    const fn bit(position: u32) -> Self {
        Self(1 << (position - 1))
    }

    /// Flags from the raw `/Ff` value, keeping every bit.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw `/Ff` value.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every flag of `flags` is set.
    pub const fn contains(self, flags: Self) -> bool {
        self.0 & flags.0 == flags.0
    }

    /// The field cannot be changed interactively.
    pub const fn read_only(self) -> bool {
        self.contains(Self::READ_ONLY)
    }

    /// A value is required when exporting.
    pub const fn required(self) -> bool {
        self.contains(Self::REQUIRED)
    }

    /// The field is excluded from export.
    pub const fn no_export(self) -> bool {
        self.contains(Self::NO_EXPORT)
    }
}

/// Button interaction kind (12.7.5.2, Table 229); the table closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// A momentary control, without a permanent value.
    Push,
    /// A check box.
    CheckBox,
    /// A group of radio widgets; [`FieldFlags::RADIOS_IN_UNISON`] controls
    /// shared on states.
    Radio,
}

/// Quadding for variable text (Table 228); `Q` has exactly these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Justification {
    /// Left (the default).
    #[default]
    Left,
    /// Centered.
    Center,
    /// Right.
    Right,
}

/// How a choice field presents its options (12.7.5.4, Table 233).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceStyle {
    /// A scrollable list box.
    List,
    /// A drop-down combo box.
    Combo {
        /// It also accepts typed text ([`FieldFlags::EDIT`]).
        editable: bool,
    },
}

/// One choice option, retained in source order (Table 234).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChoiceOption {
    /// Export text; equal to display for the single-string form.
    pub export: String,
    /// Display text.
    pub display: String,
}

/// Typed terminal field configuration; nonterminal nodes have no logical type.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
        /// List box or combo box.
        style: ChoiceStyle,
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
#[non_exhaustive]
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
#[non_exhaustive]
pub struct AdditionalAction<'a> {
    /// Trigger name, preserving extension events too.
    pub trigger: &'a Name,
    /// Typed inert action/Next sequence.
    pub actions: ActionSequence<'a>,
}

/// A field node and its effective inherited attributes (12.7.4.1).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub struct Widget<'a> {
    pub(super) has_appearance: bool,
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
#[non_exhaustive]
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
