//! Stable tags for reported content repairs (ADR 0003). This crate owns
//! the `content.` area, and the annotation and Type 3 repairs that content
//! interpretation applies.

use pagura_core::Repair;

/// Table 73: numeric SC/sc operands are interpreted as SCN/scn only
/// when their count and types fit the selected non-pattern space.
pub const COLOR_OPERATOR: Repair = Repair::new("content.color-operator");
/// 8.5.3: an undefined path paints nothing; any pending clip is discarded.
pub const EMPTY_PATH: Repair = Repair::new("content.empty-path");
/// 8.5.4: defer clipping until painting, including subsequent construction.
pub const CLIP_ORDER: Repair = Repair::new("content.clip-order");
/// 8.6.6.3: ignore bytes beyond the complete, declared palette.
pub const INDEXED_EXCESS: Repair = Repair::new("content.indexed-excess");
/// Table 98: ignore optional-content structures without a default config.
pub const OPTIONAL_CONFIG: Repair = Repair::new("content.optional-config");
/// Table 98: ignore optional-content structures without a group registry.
pub const OPTIONAL_GROUPS: Repair = Repair::new("content.optional-groups");
/// 8.11.4.2: ignore optional structures when required OCProperties is absent.
pub const OPTIONAL_PROPERTIES: Repair = Repair::new("content.optional-properties");
/// 8.11.4.3: use the initial ON state for an unregistered, valid OCG.
/// This also applies under OCMD negation; readers differ on this policy.
pub const OPTIONAL_GROUP: Repair = Repair::new("content.optional-group");
/// 9.3.6: retain accumulated glyph clips through ET after a nonclipping reset.
/// Most readers keep the clip when recovering this prohibited transition.
pub const TEXT_CLIP_RESET: Repair = Repair::new("content.text-clip-reset");
/// 7.9.5; 12.5.5: skip an appearance with a zero-sized source or target
/// rectangle. Most readers paint nothing where fitting has no area.
pub const APPEARANCE_EMPTY: Repair = Repair::new("annotation.appearance-empty");
/// 7.8.2; 8.4.2: discard unmatched saved graphics states at the end of a
/// content sequence, after all painting and text objects have finished.
pub const GRAPHICS_UNCLOSED: Repair = Repair::new("content.graphics-unclosed");
/// 14.6: close marked-content sequences left open at the end of a content
/// sequence, after all painting and text objects have finished.
pub const MARKED_UNCLOSED: Repair = Repair::new("content.marked-unclosed");
/// 9.6.4, Table 111: retain the horizontal Widths advance when d0/d1 disagrees.
pub const TYPE3_WIDTH: Repair = Repair::new("font.type3-width");
/// 8.7.3.2, Table 73: coloured patterns require only a name; discard extra
/// numeric operands supplied with an underlying pattern colour space.
pub const PATTERN_UNDERLYING: Repair = Repair::new("content.pattern-underlying");

/// 8.6.5.5: an ICC profile whose data cannot be used (damaged, malformed or
/// unreadable) is treated as unsupported: the Alternate space, or the default
/// for `N`, draws instead. Budget errors are never repaired.
pub const ICC_UNUSABLE: Repair = Repair::new("content.icc-unusable");

/// Every tag above.
pub const ALL: &[Repair] = &[
    COLOR_OPERATOR,
    EMPTY_PATH,
    CLIP_ORDER,
    INDEXED_EXCESS,
    OPTIONAL_CONFIG,
    OPTIONAL_GROUPS,
    OPTIONAL_PROPERTIES,
    OPTIONAL_GROUP,
    TEXT_CLIP_RESET,
    APPEARANCE_EMPTY,
    GRAPHICS_UNCLOSED,
    MARKED_UNCLOSED,
    TYPE3_WIDTH,
    PATTERN_UNDERLYING,
    ICC_UNUSABLE,
];
