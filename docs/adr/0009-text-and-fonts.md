# ADR 0009: Text and fonts

Status: accepted, 2026-10-17. Amended by ADR 0025 (descriptor flags are normalized in `pagura-text`).

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
- **Symbol and ZapfDingbats** (amended 2026-10-18): FoxitSymbol and
  FoxitDingbats, PDFium's BSD-3-Clause CFF fonts as pdf.js redistributes
  them. They are metric-compatible and carry the AFM glyph names, so glyphs
  are selected by name through the built-in encoding or Differences.
  Copyleft sets such as URW base-35 remain excluded.
- **Non-embedded CID fonts** (amended 2026-10-18, opt-in `cjk-fonts`):
  Source Han Sans Regular for the Japan1, GB1, CNS1 and Korea1 collections
  (SIL OFL 1.1), about 23 MiB. CIDs reach a glyph through the collection's
  Unicode data and the font's Unicode cmap, never through the font's own
  CIDs, and the PDF's W and DW keep the advances. Writing is horizontal
  only until vertical forms are supported (`font.cid-substitute-vertical`).
  The feature is off by default, so default binaries and the WebAssembly
  host stay small.

### Footprint and features (amended 2026-10-18)

Each `pagura-data` feature links one set of files. The sizes are those of
the linked files, which the binary carries uncompressed:

| Feature | Files | Linked | Default |
|---|---|---|---|
| `standard-metrics` | 14 Core14 AFM files | 0.6 MiB | yes |
| `standard-outlines` (implies `standard-metrics`) | 12 croscore TrueType fonts, 2 Foxit CFF fonts | 5.8 MiB | yes |
| `cmaps` | 59 Table 116 CMaps, 4 `cid2unicode.txt` | 3.4 MiB | yes |
| `cjk-fonts` (implies `cmaps`) | 4 Source Han Sans OpenType fonts | 22.3 MiB | no |

What each configuration carries:

- **The default WebAssembly host** (`pagura-host` without features, which
  the web viewer loads) links no data. Its fonts reach it through a
  host-side provider or not at all.
- **The facade's default `bundled-data` feature** takes `pagura-data`'s
  defaults. It links 9.8 MiB into the `pagura` CLI and into a
  `pagura-host --features bundled-data` module.
- **`cjk-fonts`** adds 22.3 MiB.
- **Metrics without outlines** (`default-features = false, features =
  ["standard-metrics", "cmaps"]`) links 4.0 MiB. A non-embedded standard
  font then keeps its AFM widths and descriptor, but decoding its glyphs
  fails explicitly with `font.non-embedded`.

The outlines stay in the default set for two reasons. `bundled-data`
promises substitute outlines. And without them, every non-embedded
standard 14 font would fail to draw in default builds. The facade
offers `bundled-data` only as a whole; a metrics-only facade feature waits
for a host that needs it. ADR 0026 records how the data crate relates to
the parsers.

`pagura-text` defines the provider interface: a standard font by base name,
a predefined CMap by name, and a CID substitute by character collection. A data crate implements it over the bundled files,
which are recorded with source URLs and SHA-256 in a lock, like the corpora.
Hosts may supply their own provider; the browser can fetch data lazily
instead of embedding it in the WebAssembly module. Substitution is reported
as a diagnostic, never silent, and a document's own embedded program always
wins.

### Non-embedded Identity CID fonts with Latin names (amended 2026-10-27)

Word processors write non-embedded `CIDFontType2` fonts such as ArialMT or
TimesNewRomanPSMT with `/Encoding /Identity-H` and an `Adobe-Identity` or
missing `CIDSystemInfo`. Their CIDs are glyph indices of the absent program,
so no collection Unicode identifies a glyph.

When the CIDFont's `/BaseFont` names a known Latin family, through the same
bounded standard aliases simple fonts use, the bundled metric-compatible
face (Arimo, Tinos or Cousine) draws the font:

- Each code's text in the font's own `ToUnicode` map selects the glyph
  through the face's Unicode `cmap`. This is the one place where ToUnicode
  selects glyphs (9.10.3 reserves it for extraction): for these fonts it is
  the document's only statement of what each code shows.
- Codes without ToUnicode text, notdef among them, and ligature texts paint
  nothing. A missing or discarded ToUnicode map draws nothing at all.
- The PDF's `W` and `DW` keep every advance, so glyph positions are the
  document's own.
- The substitution is reported as `font.substituted`, and the incomplete
  coverage as `font.cid-glyph-missing`. Vertical writing stays unsupported.

Other families, including unknown or non-Latin names, keep the refusal.
