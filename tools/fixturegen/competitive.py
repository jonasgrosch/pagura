"""Measured bounds of render fixtures under the competitive accuracy path.

ADR 0004's competitive accuracy gate lets an approved approximate path change
a fixture only when the fixture records that path's measured bound. Each entry
is the largest channel difference from the fixture's independent oracle under
`RenderOptions::accuracy = Accuracy::Competitive`, for the fixtures where it
exceeds the exact path's tolerance. Every other render fixture keeps its exact
tolerance under that path too. The bounds are listed with the amendment.
"""

BOUNDS = {
    # One-round byte composition: a translucent or partial mark rounds its
    # source and the stored result once each.
    "classic/alpha-unsupported": 1,
    "content/alpha": 1,
    "paint/image-direct-rgb-fractional": 1,
    "paint/text-knockout-false": 1,
    "zero_area/half-alpha": 1,
    # Byte storage accumulates one rounding per mark over stacks of partial
    # marks on one pixel.
    "content/alpha-accumulation": 66,
    "content/precision-translucent-stagnation": 127,
    # Small symbol glyphs from row fragments at snapped origins.
    "paint/bundled-symbol": 25,
    "paint/bundled-symbol-differences": 27,
    "paint/bundled-zapfdingbats": 14,
    "paint/bundled-zapfdingbats-differences": 19,
    # A zero-width shading BBox's boundary outline in signed cells.
    "zero_area/zero-width-shading-bbox": 64,
}
