# Architecture

## Crates and dependency direction

```
pagura-cli       ──► pagura (the `pagura` command-line tool; the facade only)
pagura-host      ──► pagura
pagura           ──► pagura-render, pagura-appearance (form appearances), pagura-write, pagura-document, pagura-extract (page text), pagura-text and pagura-font (font kinds, provider types), pagura-raster (`Pixmap`), pagura-interpret, pagura-color and pagura-image (tags), pagura-pki (CMS signature verification), pagura-data (optional bundled fonts and CMaps), pagura-crypto (Unicode data notice)
pagura-render    ──► pagura-paint, pagura-interpret, pagura-extract, pagura-text, pagura-document, pagura-raster
pagura-paint     ──► pagura-interpret, pagura-color, pagura-image, pagura-raster
pagura-interpret ──► pagura-content, pagura-appearance, pagura-color, pagura-image, pagura-raster, pagura-extract, pagura-text, pagura-font, pagura-document
pagura-appearance ─► pagura-text, pagura-document
pagura-extract   ──► pagura-text (font kinds, writing modes, glyph text), pagura-document, pagura-raster
pagura-color     ──► pagura-crypto (MD5 for ICC profile IDs)
pagura-data      ──► pagura-text (its Provider), pagura-font (ADR 0026)
pagura-text      ──► pagura-font, pagura-document, pagura-raster
pagura-image     ──► pagura-document, pagura-codec
pagura-font      ──► pagura-raster
pagura-document  ──► pagura-codec, pagura-crypto
pagura-write     ──► pagura-crypto (SHA-256 for revision identifiers)
every crate      ──► pagura-core
```

Dependencies point downward only and are fixed in the crate manifests.
`pagura-reference` (the slow, obviously correct renderer) is a test-only
oracle: only dev-dependencies name it.

| Crate | Responsibility | Knows nothing about |
|---|---|---|
| `pagura-core` | `Value`, `Name`, `Dictionary`, `ObjectId`, `Rect`; bounded `syntax::Parser`; `Error`/`ErrorKind`/`Feature`/`Position`; `Diagnostic`/`DiagnosticKind`/`Repair`/`Recovery`; `Limits`; the primitives every parser shares (ADR 0017): `Meter`, `bytes::{Cursor, BitCursor}`, `text_string` | files, revisions, rendering |
| `pagura-codec` | stream filters and predictors with streaming output caps | documents, references |
| `pagura-crypto` | `Digest` hashes (MD5, SHA-1, SHA-2), RSA and ECDSA signature verification; the standard security handler from plain parameters, with RC4 and AES internal to it (ADR 0021) | the object model beyond `ObjectId`; signing; certificates and trust |
| `pagura-document` | immutable source bytes, cross-reference data and revisions, lazily parsed and cached objects, decoded streams, typed page tree, recovery; navigation (12.3–12.6), interactive-form views and in-memory filling (12.7) and signature structure (12.8) as inert views | content streams, pixels, executing actions, cryptographic verification |
| `pagura-raster` | geometry, paths, flattening and stroke outlines; the band rasterizer: fills, strokes, glyph placements and clips prepared once per render and rasterized band by band into exact-area coverage; the `Pixmap` output surface and the one float-to-byte quantizer (ADR 0022) | PDF |
| `pagura-font` | font programs (TrueType/OpenType, CFF/CFF2, Type 1) as outlines, advances, encodings and cmaps | PDF dictionaries |
| `pagura-data` | optional bundled data for fonts a PDF does not embed: standard-14 metrics and substitute outlines, predefined CMaps, CID-to-Unicode data and CJK substitutes, behind one cargo feature each, as a `Provider` with lazy caches (ADR 0009, 0026) | documents, content streams |
| `pagura-text` | PDF font dictionaries, encodings, widths, CMaps and ToUnicode, descriptor flag normalization; shown strings to glyphs (ADR 0009, ADR 0025) | content streams, pixels, page layout |
| `pagura-extract` | page text: layout policy, reading order, search and the logical structure view over the interpreter's positioned glyphs (ADR 0014, ADR 0025) | content streams, font programs, pixels |
| `pagura-image` | image XObjects and inline images as sources read without decoding, decoded on request (layout and orientation); JPEG, CCITT, JBIG2 and JPEG 2000 decoding to samples | painting, colour conversion |
| `pagura-color` | colour spaces, prepared converters, ICC profile transforms and CIE colorimetry; PDF functions, prepared and total (ADR 0028, ADR 0034) | PDF objects |
| `pagura-content` | content-stream tokenizer and typed operators (Annex A) with source positions | graphics state, resources |
| `pagura-interpret` | the one interpreter: the graphics state machine over pages, forms, annotations and patterns into a `Device`, with `Needs` negotiation; the scene recorder and the text device (ADR 0034) | pixels, resolution |
| `pagura-appearance` | form-field and annotation appearance generation, as appearance streams kept as data (12.5.5, 12.7.4.3) | interpretation, pixels |
| `pagura-paint` | the renderer: preparation and reservation of a scene for one region, the band executor, prepared paint, span kernels and storage formats (ADR 0034) | PDF objects, documents |
| `pagura-render` | the thin rendering layer: `RenderContext` with its `Store`, `RenderOptions`, page recording, rendering by region and text extraction | interpretation and painting internals |
| `pagura-reference` | test only: the slow, obviously correct f64 rasterizer and compositor for differential tests, and the exact coverage of single shapes for the band rasterizer's tests | product code |
| `pagura-write` | deterministic serialization | parsing |
| `pagura` | the public facade: every public type, and the free functions with the feature-selected font data | — |
| `pagura-cli` | the `pagura` command-line tool, built on the facade alone (ADR 0030) | engine crates |
| `pagura-host` | the stdio session protocol, native and `wasm32-wasip1` | — |
| `pagura-testkit` | test-only fixture access, manifest reading, byte assertions and host protocol frames (std only) | product code |
| `pagura-fuzz` | test-only fuzz drivers, shared by the libFuzzer targets in `fuzz/` and the stable replay of `fuzz/regressions/` (ADR 0030) | product code |

Add a crate only for a substantial, independently testable responsibility
with a clear dependency direction, as `pagura-color` is for colour
management (ADR 0028).

## Invariants

1. Original bytes are immutable. A no-op save can return the original bytes.
2. Object references are document-relative, generation-aware identities. A
   reference to an undefined object resolves to null (clause 7.3.10).
3. Every traversal has an explicit budget; every decoder enforces an output
   cap while decoding; the storage a document retains in its caches has an
   aggregate budget (`Limits::document_bytes`, ADR 0012). Each `Limits`
   field bounds exactly one documented quantity and is charged through
   `pagura_core::Meter` (ADR 0016).
4. A rendering error cannot become a successful partial page.
5. Unsupported behaviour is an `Unsupported(Feature)` error with a stable tag;
   diagnostics distinguish repairs from information; only repairs carry `Repair`
   tags and require recovery permission (ADR 0003).
6. Unknown objects remain accessible for future preservation; typed views
   (`Page`) never replace the underlying document model.
7. Interpretation has no host capabilities. Fonts, trust stores, network range
   reads and user actions enter through explicit providers. In WebAssembly the
   runtime grants no filesystem or network at all (ADR 0005).
8. No global mutable state. A `Document` is `Send + Sync`; its caches
   (`OnceLock` per object slot, the page list) fill lazily and never change
   what a lookup returns. Future caches are keyed by revision, resource
   identity and output settings.

## Document model

`Document::open` reads the header, `startxref`, cross-reference data and
trailer, producing one slot per in-use object. `object(id)` parses on first
access and caches; `resolve(value)` follows references; `stream_data` applies
decryption (security handler) and then the filter chain (`pagura-codec`).
`pages()` walks the page tree once, applying inheritance, normalizing boxes,
intersecting the crop box with the media box, and validating `/Rotate` and
`/UserUnit`.

## Rendering

See ADR 0034 and ADR 0004. One interpreter (`pagura-interpret`) runs a page's
content, forms and annotations into a `Device`, which declares its `Needs`
first. The scene recorder records a device-independent `Scene`: marks in
source order with interned paints, composite states and clips, colours in
their source spaces and images as undecoded sources. A `RenderContext` keeps
each page's scene in its `Store`, one byte capacity for everything rendering
reuses: scenes, decoded images, colour transforms, font programs and glyph
edge setups (ADR 0012).

Rendering a region prepares the scene for it (`pagura_paint::prepare`):
in source order it culls marks by conservative device bounds, validates them,
reserves every budget (ADR 0016), builds coverage
sources and prepared paint, chooses each buffer's storage format from its
group's whole content and bins marks to bands of the global device grid.
Coverage comes from the band rasterizer (`pagura-raster`): each mark's edges
accumulate signed areas in f64 cells, certified per shape or resolved per
cell where the winding needs it, so every coverage byte is the exact area
rounded once (ADR 0004, section 4). The band executor then cannot fail; it can
stop between bands for a viewer. Each output pixel is quantized once onto the
background premultiplied once. Output does not depend on band height, tiles or
platform.

Once the sequential original-curve collector has certified every coverage
byte of a glyph placement, preparation may retain complete coverage rows
instead of rebuilding its polygon. This optional construction borrows only
unused work from the completed inspection and the byte meter's remaining
capacity. It preserves the shape's validation bounds and publishes only when
its actual storage and complete execution work fit conservative lower bounds
for the ordinary replacement. A declined attempt releases all its owners
before that replacement continues. The rows are an authoritative coverage
source for the common band executor; they retain no old geometry, correction
map or proof oracle and do not enter the document store.

One band driver either appends output directly into an owned `Pixmap` or
lends a reusable RGBA band to a consumer. Output storage belongs to the call:
`decoded_bytes` caps the full collected surface or the streamed band, while
`pixels` still caps the whole requested region. Preparation, repair reporting,
scratch fitting and output reservation finish before the first consumer
call. Paint execution cannot fail; a consumer can return its own error, such
as an I/O failure, after output starts. A stop between bands is not an error.

Rendering is exact: every pixel within one level of its exact result (ADR
0004). Glyphs are rasterized at their exact device positions (ADR 0013).
The reference renderer (`pagura-reference`) is the diagnostic oracle, not an
infallible one: its harness compares the renderer with it over every
conformance render case and checks pixels, errors and charges across band
heights, partitions and store states (`tools/check.py` runs it in release);
the band rasterizer's fills, strokes, clips and glyphs are checked against
its exact single-shape coverage.

## Reading (M6)

See ADR 0014.
- **Text:** the interpreter's text device collects positioned glyphs without
  decoding images, parsing shadings or converting colours. `pagura-extract`
  lays them out into words, lines and a reading order and searches them;
  replacement text follows 14.9.4, and the rest is documented policy. Tagged pages also get
  a separate logical view in structure order (14.7–14.8) for assistive
  technology; search and selection never depend on it.
- **Navigation:** `pagura_document::navigation` reads outlines, destinations,
  links, actions and page labels as bounded views. Actions are inert data: the
  engine never follows a URI, opens a file or runs a script, and the host
  decides.

## Editing (M7)

`pagura::editing::Transaction` owns a bounded overlay over an immutable
`Document`. Staging and rollback never invalidate source caches; commit returns
new bytes to reopen as another snapshot. `pagura-write` owns shared syntax
validation and classic/xref-stream append serialization. The facade owns source
policy, generation-aware references and filling orchestration, reusing the
existing appearance generator (ADR 0031). Full rewrite, undo/redo, signature
permission enforcement and sanitizing redaction have separate future contracts.
A rendered scene is never the sole editing representation. Modified incremental
saves require strict input; repair-mode inputs require a future full rewrite.

## Performance policy

Speed is earned without spending quality. Every optimization leaves the code at
least as clean as it found it, and the preferred levers, in order, are:

1. **Do less work by acting smarter**: a better algorithm, a representation that
   makes the common case cheap, work avoided or done once (caching, sharing,
   early rejection) rather than done faster.
2. **Simplify**: less code, fewer special cases and a cleaner architecture. A
   simpler design that is also faster is the ideal result.
3. **Then improve data layout, scheduling and locality**, so the compiler can
   vectorize (SIMD128 through autovectorization in WebAssembly).

Added complexity must be defended against the speed it buys: the change records
the measured gain, the code it adds or removes and the alternatives considered,
and a small gain does not justify a large or fragile mechanism. Profile before
optimizing, and keep algorithmic fallbacks explicit and bounded. Never trade
fidelity, security, portability or the no-unsafe rule for speed.

Before choosing a performance experiment, establish a comparison on the same
inputs and host, with pinned engine versions, settings, output checks and cache
lifetimes. Measure cold and warm requests separately. Capture phase timings and
profiles for both implementations, distinguishing inclusive from exclusive
costs. Where their work differs, count that work: geometry produced, repeated
preparation, intersection candidates, fallbacks and allocations. A hotspot in
one implementation is not yet an explanation of the gap.

Rank experiments by the end-to-end cost they can remove and state the evidence
that would reject each hypothesis. Prefer removing dominant or repeated work
over making a small operation cheaper. Reuse the comparison harness and collect
related experiments in bounded sessions. Keep instrumented traces and counts
separate from timings of the default build. Run focused correctness checks
before measuring a candidate; run the complete required correctness and
platform gates before retaining it. Preserve failed trials and report control
regressions alongside gains.

An optimization keeps output identical, or stays within the rendering
accuracy contract of ADR 0004: every pixel within one level of its exact
result. A change that alters output qualifies only under these conditions:

- an independent exact oracle covers the affected output;
- the bound holds on every fixture, benchmark page and stress case,
  including cases where many partial-coverage marks overlap;
- the change reports every changed pixel with its oracle value;
- it reports how often the old and new outputs match the exact rounding, so
  that a systematic loss of precision is visible.

A fixture's tolerance may rise to 1 only for such a change, and each one is
listed with its case. Anything beyond one level is a correctness change, not
an optimization, and needs its own decision.

When an area stalls, study how the best-in-class implementation of that
specific problem works (ADR 0006: inspiration, never copied or translated code),
then build something cleaner, more idiomatic and faster than it.

Architecture-specific native builds are a separately labeled benchmark track
with identical competitor tuning.
