# ADR 0025: Page text in `pagura-extract`; descriptor flags in `pagura-text`

Status: accepted, 2026-10-18. Amends ADR 0009 (where font-dictionary repairs
live) and ADR 0014 (which crate lays out page text).

## Context

ADR 0009 gives `pagura-text` the interpretation of PDF font dictionaries:
encodings, widths, CMaps, ToUnicode and the choice of a font program. A
workspace review found two places where the code had drifted from that:

- **A second responsibility inside `pagura-text`.** About 2,800 lines in
  `src/page/` lay out positioned glyphs into lines and a reading order,
  search them, and read the logical structure tree (ISO 32000-2 9.10,
  14.7–14.9; ADR 0014). They consume records that the content interpreter
  in `pagura-render` produces, and share nothing with font-dictionary
  interpretation except the `FontKind` and `WritingMode` enums. The crate
  table in `docs/ARCHITECTURE.md` did not mention them.
- **Font-dictionary policy inside `pagura-render`.** About 500 lines in
  `pagura-render/src/interpreter/text/cache/flags/` normalized the
  Symbolic and Nonsymbolic descriptor flags (9.8.2, Table 121). Because
  `PdfFont::load` rejected contradictory flags without a repair route, the
  renderer rewrote a corrected copy of the font dictionary, loaded the font
  twice, and probed `decode(&[0])` and matched
  `Unsupported(font.non-embedded)` to find out whether the font had a
  program. Extraction-only callers of `PdfFont` got none of it.

## Decision

1. **A new crate, `pagura-extract`, owns page text:** `TextPage`, layout
   options and policy, reading order, search, `LogicalText` and the
   structure reader, the positioned-text records the interpreter fills, and
   the `text.` repair tags (`text.marked-property`,
   `text.structure-fallback`). It depends on `pagura-core`,
   `pagura-document`, `pagura-raster` (for `Point`) and `pagura-text` (for
   `FontKind`, `WritingMode` and `GlyphText`). `pagura-render` depends on it
   to fill the records and re-exports `TextPage`, `SearchOptions` and
   `TextMatch`, so the facade's names are unchanged.
2. **`pagura-text` is font dictionaries only:** loading a `PdfFont` and
   splitting shown strings into glyphs. It owns the `font.` tags that
   describe PDF font dictionaries, including `font.descriptor-flags`.
3. **Descriptor flags are normalized inside `PdfFont::load`.** Flags are
   read once into a `FontFlags` value. Valid Symbolic/Nonsymbolic pairs keep
   their 9.6.5.2/9.6.5.4 selection routes unchanged. A pair that is both set
   or both clear, or a missing or non-integer `/Flags`, is reported as
   `font.descriptor-flags`, keeping Symbolic as the primary bit; strict mode
   rejects it. This includes a non-embedded CIDFont that a provider
   substitutes. `PdfFont::load_at` locates the repair at the operator that
   selected the font. Source objects are never rewritten and the font is
   loaded once.
4. **`PdfFont::program()`** returns the embedded or substituted outline
   program, so callers never probe glyph decoding or error kinds to learn
   whether one exists.

## Consequences

The dependency graph gains one edge layer:
`pagura-render ──► pagura-extract ──► pagura-text`. Font-dictionary changes
no longer rebuild the layout and structure code, and each crate's tests
cover one responsibility. Callers that imported page-text types from
`pagura_text` import them from `pagura_extract`; facade users see no change.
Every caller of `PdfFont`, including text extraction without rendering,
gets the same descriptor repair, reported once per font load.
