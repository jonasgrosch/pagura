# ADR 0013: Keep exact glyph placement; no glyph coverage cache

Status: accepted, 2026-10-18.

## Context

Glyph outlines are filled through the same exact-area rasterizer as paths
(ADR 0004, ADR 0009), at their exact device positions. On text-heavy pages,
building and rasterizing glyph masks takes most of the render time.

Fast engines cache a glyph's coverage mask per font, glyph, size and
transform, and reuse it for every occurrence. FreeType, Skia, PDFium and
MuPDF quantize glyph positions to a sub-pixel grid for this purpose; MuPDF
documents the phase quantization. A cached mask can be reused only at the same
sub-pixel phase, so reuse means snapping each glyph to a grid and giving up
exact placement.

A prototype measured the trade-off. It ran over the 40 benchmark page and DPI
combinations and 358 PDF fixtures at two resolutions, with deterministic LRU
eviction and a 4 MiB cache. These are exploratory measurements on one shared
machine, not a performance claim (ADR 0002). Speed-ups are median per-page
render-time ratios:

| Positioning | Text-heavy, 72 dpi | Text-heavy, 150 dpi | Mixed, 72 dpi | Mixed, 150 dpi |
|---|---:|---:|---:|---:|
| Exact, reused only on identical phase | 1.00× | 1.02× | 1.01× | 1.00× |
| 1/4 pixel | 1.66× | 1.37× | 1.08× | 1.06× |
| 1/16 pixel | 1.16× | 1.09× | 1.05× | 1.03× |
| 1/64 pixel | 1.04× | 1.06× | 1.02× | 1.01× |

Differences from exact placement, in RGBA8 levels:

| Positioning | Maximum, fixtures / pages | Mean, fixtures / pages | Changed pixels, fixtures / pages |
|---|---:|---:|---:|
| 1/4 pixel | 37 / 255 | 0.152 / 0.485 | 1.16% / 5.45% |
| 1/16 pixel | 10 / 255 | 0.038 / 0.120 | 1.16% / 4.69% |
| 1/64 pixel | 2 / 4 | 0.008 / 0.029 | 1.07% / 3.28% |

The prototype added 368 lines. Its cache also raised peak native memory on the
pages checked.

## Decision

Glyphs keep their exact device positions, and the renderer has no glyph
coverage cache:

- Exact-phase reuse saves nothing measurable, because exact positions almost
  never repeat.
- The only grid with a substantial gain (1/4 pixel) changes about 5% of a real
  page's pixels, some by the full range.
- The grid that is nearly invisible (1/64 pixel) gains at most 6%.

No grid buys enough speed to justify its fidelity cost and its code.

Text speed comes from the rasterizer instead, following the performance policy
in `docs/ARCHITECTURE.md`: a faster exact-area rasterizer helps glyphs and every
other fill at no cost in fidelity.

## Consequences

- Every glyph is rasterized at its exact position, so output stays independent
  of tile layout, cache state and memory budget.
- Text-heavy pages depend on rasterizer throughput.
- Revisit this decision only with new measurements. One reason would be a
  faster rasterizer that leaves glyph rasterization dominant. Another would be
  a target that renders the same text at the same scale many times, such as
  repeated viewer tiles. Any grid is then an explicit rendering policy with
  independent positioning oracles. Its phase is computed in global device
  coordinates, so tiles stay identical, and its cache is bounded and charged
  to the retained budget (ADR 0012).
