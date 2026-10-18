//! Stable unsupported-capability tags for interactive forms.
use pagura_core::Feature;
/// XFA interpretation and filling.
pub const XFA: Feature = Feature::new("form.xfa");
/// Rich text generation and filling.
pub const RICH_TEXT: Feature = Feature::new("form.rich-text");
/// Filling unsupported or nonterminal field types.
pub const FIELD_TYPE: Feature = Feature::new("form.field-type");
