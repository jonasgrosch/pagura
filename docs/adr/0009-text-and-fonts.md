# ADR 0009: Text and fonts

Status: accepted, 2026-10-17.

## Decision

Text is three layers with one-directional dependencies:

1. **`pagura-font`** parses font programs — Type 1 (eexec, charstrings),
   bare and CID-keyed CFF, TrueType `glyf`, OpenType with CFF/CFF2 — into
   glyph outlines (`pagura_raster::Path` in glyph space), advances, charsets,
   `cmap` subtables and built-in encodings. It is PDF-agnostic and applies no
   hinting. Charstring interpreters are bounded (stack, subroutine depth,
   operator count).
2. **`pagura-text`** interprets PDF font dictionaries (clause 9): simple
   fonts with encodings and `/Differences`, `/Widths`, Type 3 fonts, and
   Type 0 fonts with CIDFontType0/2 descendants, embedded CMaps, Identity-H/V,
   `/W`, `/W2`, `/CIDToGIDMap` and `/ToUnicode`. `PdfFont::decode` turns a
   string operand into glyphs with advances, word-spacing eligibility,
   Unicode and a drawable shape.
3. **`pagura-render`** owns text state and positioning (clause 9.3, `Tm`,
   `TJ` adjustments, `Tc`, `Tw`, `Tz`, `TL`, `Ts`), render modes 0–7 including
   text clipping, Type 3 glyph procedures (`d0`/`d1`), and draws outlines
   through the same exact-area rasterizer as paths.

Fonts are loaded once per document and font dictionary and shared; glyph
outlines are cached by program and glyph. Unicode text is retained for
extraction and search (M6).

## Open decisions

- **Non-embedded fonts** (including the standard 14) need bundled substitute
  fonts and metrics. Their licences and provenance require a separate
  decision under ADR 0008; until then they fail with `font.non-embedded`.
- **Predefined CJK CMaps** (beyond Identity-H/V) need bundled CMap data;
  until decided they fail with `font.predefined-cmap`.
