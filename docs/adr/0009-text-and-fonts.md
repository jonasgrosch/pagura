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
| `cjk-fonts` (implies `cmaps`) | 4 Source Han Sans OpenType fonts | 22.3 MiB | no (CLI: yes) |

What each configuration carries:

- **The default WebAssembly host** (`pagura-host` without features, which
  the web viewer loads) links no data. Its fonts reach it through a
  host-side provider or not at all.
- **The facade's default `bundled-data` feature** takes `pagura-data`'s
  defaults. It links 9.8 MiB into the `pagura` CLI and into a
  `pagura-host --features bundled-data` module.
- **`cjk-fonts`** adds 22.3 MiB. It stays off in the library and the
  WebAssembly host, which keeps the viewer module small.
- **The `pagura` CLI** (amended 2026-10-27) enables `cjk-fonts` by default,
  so it links 32.1 MiB. A command-line tool is judged by the documents it
  renders, and without the substitutes every non-embedded Japan1, GB1, CNS1
  or Korea1 font fails. Builds that need the smaller binary use
  `--no-default-features --features bundled-data`. Every substitution is
  reported as `font.substituted`, as in any other configuration.
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

### System CJK fonts in a host (amended 2026-10-27)

A host without `cjk-fonts`, such as the WebAssembly viewer or an
application that ships its own fonts, supplies CJK substitutes through its
own `Provider`:

- `cid_font(registry, ordering)` returns a font the host already has, such
  as a system Noto Sans CJK, Hiragino or Source Han face, parsed with
  `FontProgram::parse` under the provider's `Limits`. It needs a Unicode
  `cmap` (Windows Unicode BMP or full repertoire), and returns `None` for
  collections the host does not cover. The program's own CIDs are never used.
- `cid_to_unicode(registry, ordering)` returns the collection's CID to
  Unicode data. A host with `bundled-data` delegates to the bundled
  provider, which links the four `cid2unicode.txt` files without any CJK
  font. Otherwise it builds the table with `CidToUnicode::new`.
- `predefined_cmap` and `standard_font` delegate to the bundled provider or
  return `None`.

Selection and reporting do not change: glyphs are chosen through Unicode,
the PDF's `W` and `DW` keep the advances, missing glyphs paint nothing and
are reported once as `font.cid-glyph-missing`, and the font is reported as
`font.substituted`. Vertical writing still needs vertical forms
(`font.cid-substitute-vertical`).

### Symbolic flags on non-embedded Latin fonts (amended 2026-10-27)

A non-embedded font flagged Symbolic leaves the glyph of each code to the
original program's own encoding (9.6.5.2, 9.6.5.4). A substitute cannot know
it, so Pagura makes no guess for such fonts, with one exception. When the
font names a known Latin family and its `/Encoding` names, or has a
`/BaseEncoding` naming, `StandardEncoding`, `WinAnsiEncoding` or
`MacRomanEncoding`, that encoding is the document's own statement of its
glyph names. Form generators commonly write such fonts, for example
LiberationSans or ArialMT with `/Flags 4` and `/WinAnsiEncoding`.

- The encoding is followed, and the Symbolic flag that contradicts it is
  the reported repair `font.symbolic-latin-encoding`. Strict mode rejects it.
- The family is known when the bounded standard aliases name a Latin face
  (Arial, Times New Roman, Courier New and their spellings), which then
  selects the face. A short list of Latin text families also counts: the
  metric-compatible Liberation and croscore faces, and DejaVu. Their face
  comes from the descriptor, as for any unknown nonsymbolic font.
- Symbol, dingbat and unknown families, fonts without an explicit Latin
  encoding, and embedded programs keep their 9.6.5 routes. This narrows
  ADR 0025's rule that valid flag pairs keep their selection routes, for
  non-embedded substitutes only.

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

as a diagnostic, never silent, and a document's own embedded program wins
whenever it is usable.

### Unusable embedded programs (amended 2026-10-27)

An embedded program first goes through its parser's own reported repairs
(`font.sfnt-version`, `font.table-bounds` and the others in
`docs/RECOVERY.md`). If the program still cannot be used, because of a syntax
error, a format that disagrees with the font's binding, or an unsupported
construct, it is replaced by the substitute a non-embedded font with the same
name and descriptor would get. The same rules apply as for a non-embedded
font, including the symbolic and collection rules.

- Reported as the repair `font.embedded-unusable` at the program's error.
  Strict mode keeps the program's error with that repair attached.
- The PDF's widths (`Widths`, `W`, `DW`) keep every advance.
- Budget, cycle and other errors keep the program's error, and so do fonts
  for which no substitute exists.

### Embedded font collections (amended 2026-10-30)

TrueType/OpenType collections keep one immutable full-file owner. Table
positions remain relative to that file, including tables shared by faces;
selecting a face never extracts and rewrites a synthetic standalone font.
The native font API accepts an explicit zero-based face index or ordered
PostScript names. Its ordinary parse selects index zero. PDF bindings have
no face index: they prefer the descriptor's FontName, then BaseFont, then
those names with a valid six-uppercase-letter subset prefix removed. Each
hint must uniquely match naming-table ID 6 at its priority. Missing or
ambiguous names fail; no first-face fallback guesses the document's face.

The bounded name scan validates TTC v1/v2 headers, candidate directories and
name-table strings before parsing the selected program. Font work and string
limits apply before traversal; input storage is charged once, and only the
selected face's tables/glyph metadata are retained. Independent glyf/CFF
rectangles check both faces, shared tables, late directories and PDF routes.
PDF embedding uses the reported repair `font.collection-face`; Strict rejects
this reader selection policy beyond Table 124. Native collection parsing
remains valid in either mode. This does not claim PDF standardizes collections. Existing simple/CID mappings and PDF advances stay
in force. Unknown collection versions, ambiguous names and malformed
containers remain errors in Strict and Repair.

The selected collection face must also match Table 124's outline family:
TrueType and CIDFontType2 use glyf; Type1 uses non-CID CFF; CIDFontType0
uses CFF with either charset CIDs or direct GIDs. A contradictory face is
not loaded under the PDF's font subtype. This prevents a CID-CFF charset
from bypassing an explicit CIDFontType2 CIDToGIDMap. Existing reported
substitution remains the fallback for fonts eligible under the provider
policy. The native collection API retains its format-independent face
selection; parsed-family inspection is available separately.

Contradictory standalone sfnt/font-family bindings remain a separate
validation gap. Their existing selection policy is outside this collection
admission change.

### Contradictory CID program labels (amended 2026-10-30)

A CIDFontType0 can carry an intact CFF1 or sfnt program under the wrong
embedding key or stream subtype. Repair mode parses the complete original
bytes according to a recognized CFF1, OTTO or TrueType signature, retaining
CFF charset CIDs or defined program GIDs and the PDF's W/DW metrics. This is
reported as `font.program-binding` only after successful program validation;
Strict mode rejects it. Contradictory format-specific Length bookkeeping is
not used to truncate the decoded program. No source dictionary is rewritten.

The recovery applies only to CIDFontType0, where the existing glyph route
already uses CFF charset CIDs or direct GIDs. A TrueType replacement requires
absent/Identity CIDToGIDMap; a conflicting explicit map is not guessed away.
Unrecognized signatures, malformed programs, exhausted budgets, and genuine
Type 1 name dictionaries remain errors. Dictionary name order cannot supply
a missing CID order. Independent repaired/control rectangle pairs exercise
all admitted embedding routes; wrong Type 1 and damaged-CFF controls fail.
