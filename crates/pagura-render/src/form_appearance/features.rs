//! Stable unsupported-capability tags for interactive forms.
use pagura_core::Feature;
/// Oblique, reflected or degenerate text transforms and negative font sizes.
pub const TEXT_MATRIX: Feature = Feature::new("form.text-matrix");
/// Default appearance operators outside the supported subset.
pub const DEFAULT_APPEARANCE: Feature = Feature::new("form.default-appearance");
/// Font text without a reversible simple horizontal encoding.
pub const FONT_ENCODING: Feature = Feature::new("form.font-encoding");
/// Rounded, beveled and inset widget border generation.
pub const BORDER_STYLE: Feature = Feature::new("form.border-style");
/// Appearance generation for a rotated widget.
pub const WIDGET_ROTATION: Feature = Feature::new("form.widget-rotation");
/// Inline images, multiple variable text sections or crossed content scopes.
pub const APPEARANCE_UPDATE: Feature = Feature::new("form.appearance-update");
