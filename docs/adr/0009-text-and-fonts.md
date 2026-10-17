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

## Bundled font data (decided 2026-10-17)

Non-embedded fonts and predefined CMaps need data the PDF does not carry.
The data is bundled as **data files with their licences**, never as code,
and reaches the engine only through a provider (invariant 7):

- **Predefined CMaps** (Table 116 and the `Adobe-*-UCS2` maps used for
  Unicode): Adobe's `cmap-resources` (BSD-3-Clause), at a pinned tag.
- **Standard 14 metrics**: Adobe's Core14 AFM files, whose notice permits use,
  copying and distribution with the notice retained. They supply widths
  when a standard-14 font has no `/Widths`.
- **Standard 14 outlines**: Arimo, Tinos and Cousine (Apache-2.0), which are
  metric-compatible with Helvetica, Times and Courier, from Google's pinned
  `croscorefonts` release. A substituted glyph uses the AFM advance, so
  layout follows the PDF's metrics even where outlines differ slightly.
- **Symbol and ZapfDingbats** have no permissively licensed metric-compatible
  outlines. Copyleft sets such as URW base-35 cannot be bundled in an MIT/Apache
  project, so these fonts keep failing with `font.non-embedded` until a
  permissive source exists. Their AFM metrics are still bundled.

`pagura-text` defines the provider interface (standard font by base name,
predefined CMap by name). A data crate implements it over the bundled files,
which are recorded with source URLs and SHA-256 in a lock, like the corpora.
Hosts may supply their own provider; the browser can fetch data lazily
instead of embedding it in the WebAssembly module. Substitution is reported
as a diagnostic, never silent, and a document's own embedded program always
wins.
