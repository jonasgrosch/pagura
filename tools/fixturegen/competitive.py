"""Measured bounds of render fixtures under the fast parameters.

ADR 0004's competitive gate lets the fast parameters (section 9) change a
fixture only when the fixture records their measured bound. Each entry is the
largest channel difference from the fixture's independent oracle under
`RenderOptions::fast = FastParams::COMPETITIVE`, every parameter on, for the
fixtures where it exceeds the exact tolerance; each parameter alone stays
within the same bound. Every other render fixture keeps its exact tolerance
under the fast parameters too. ADR 0004's fixture table lists the bounds with
the parameter that causes each.
"""

BOUNDS = {
    # Byte storage: a translucent or partial mark rounds the stored result
    # once.
    "classic/alpha-unsupported": 1,
    "content/alpha": 1,
    "paint/image-direct-rgb-fractional": 1,
    "paint/text-knockout-false": 1,
    "zero_area/half-alpha": 1,
    # Byte storage accumulates one rounding per mark over stacks of partial
    # marks on one pixel.
    "content/alpha-accumulation": 66,
    "content/precision-slivers-254": 33,
    "content/precision-slivers-255": 33,
    "content/precision-slivers-1000": 122,
    "content/precision-slivers-270000": 127,
    "content/precision-translucent-stagnation": 127,
    # Glyph origin bins: small symbol glyphs placed at binned origins.
    "paint/bundled-symbol": 25,
    "paint/bundled-symbol-differences": 27,
    "paint/bundled-zapfdingbats": 14,
    "paint/bundled-zapfdingbats-differences": 19,
    # Approximate coverage: a zero-width shading BBox's boundary outline as
    # a clip in cells.
    "zero_area/zero-width-shading-bbox": 64,
}
