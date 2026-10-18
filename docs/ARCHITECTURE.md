# Architecture

## Crates and dependency direction

```
pagura-cli       ──► pagura, pagura-core, pagura-crypto, pagura-data, pagura-document (the `pagura` command-line tool)
pagura-host      ──► pagura
pagura           ──► pagura-render, pagura-write, pagura-document, pagura-raster, pagura-pki (CMS signature verification), pagura-data (optional bundled fonts and CMaps), pagura-crypto (Unicode data notice)
pagura-render    ──► pagura-extract, pagura-text, pagura-font (glyph outline diagnostics), pagura-image, pagura-document, pagura-raster, pagura-crypto (MD5 for ICC profile IDs)
pagura-extract   ──► pagura-text (font kinds, writing modes, glyph text), pagura-document, pagura-raster
pagura-data      ──► pagura-text (its Provider), pagura-font
pagura-text      ──► pagura-font, pagura-document, pagura-raster
pagura-image     ──► pagura-document, pagura-codec
pagura-font      ──► pagura-raster
pagura-document  ──► pagura-codec, pagura-crypto
pagura-write     ──► pagura-crypto (SHA-256 for revision identifiers)
every crate      ──► pagura-core
```

Dependencies point downward only and are fixed in the crate manifests.

| Crate | Responsibility | Knows nothing about |
|---|---|---|
| `pagura-core` | `Value`, `Name`, `Dictionary`, `ObjectId`, `Rect`; bounded `syntax::Parser`; `Error`/`ErrorKind`/`Feature`/`Position`; `Diagnostic`/`DiagnosticKind`/`Repair`/`Recovery`; `Limits`; the primitives every parser shares (ADR 0017): `Meter`, `bytes::{Cursor, BitCursor}`, `text_string` | files, revisions, rendering |
| `pagura-codec` | stream filters and predictors with streaming output caps | documents, references |
| `pagura-crypto` | MD5, SHA-2, RC4, AES; the standard security handler from plain parameters | the object model beyond `ObjectId` |
| `pagura-document` | immutable source bytes, cross-reference data and revisions, lazily parsed and cached objects, decoded streams, typed page tree, recovery; navigation (12.3–12.6), interactive-form views and in-memory filling (12.7) and signature structure (12.8) as inert views | content streams, pixels, executing actions, cryptographic verification |
| `pagura-raster` | geometry, paths, flattening, exact-area fill, stroking, masks, `Pixmap` | PDF |
| `pagura-font` | font programs (TrueType/OpenType, CFF/CFF2, Type 1) as outlines, advances, encodings and cmaps | PDF dictionaries |
| `pagura-text` | PDF font dictionaries, encodings, widths, CMaps and ToUnicode, descriptor flag normalization; shown strings to glyphs (ADR 0009, ADR 0025) | content streams, pixels, page layout |
| `pagura-extract` | page text: layout policy, reading order, search and the logical structure view over the interpreter's positioned glyphs (ADR 0014, ADR 0025) | content streams, font programs, pixels |
| `pagura-image` | image XObjects and inline images; JPEG, CCITT, JBIG2 and JPEG 2000 decoding to samples | painting, colour conversion |
| `pagura-render` | content interpretation into a display list in PDF terms; page-to-device transform; tiles; painting through `pagura-raster` | file structure |
| `pagura-write` | deterministic serialization | parsing |
| `pagura` | the public facade | — |
| `pagura-cli` | the `pagura` command-line tool (ADR 0030) | — |
| `pagura-host` | the stdio session protocol, native and `wasm32-wasip1` | — |
| `pagura-testkit` | test-only fixture access, manifest reading, byte assertions and host protocol frames (std only) | product code |
| `pagura-fuzz` | test-only fuzz drivers, shared by the libFuzzer targets in `fuzz/` and the stable replay of `fuzz/regressions/` (ADR 0030) | product code |

Add a crate only for a substantial, independently testable responsibility
with a clear dependency direction. Colour management (ICC) is the next
candidate; until then it lives in `pagura-render`.

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

See ADR 0004. Interpretation produces a display list in PDF terms with colours
in their source spaces; rasterization is PDF-agnostic exact-area coverage;
composition targets premultiplied RGBA8 tiles. Glyphs are rasterized at their
exact device positions without a coverage cache (ADR 0013). A deterministic CPU
renderer is the diagnostic reference, not an infallible oracle.
`RenderOptions::accuracy` can instead select the competitive path, an
approximation per primitive held to the competitive accuracy gate of ADR 0004.

## Reading (M6)

See ADR 0014.
- **Text:** the content interpreter optionally records positioned glyphs as
  output separate from the display list. `pagura-extract` lays them out into
  words, lines and a reading order and searches them; replacement text
  follows 14.9.4, and the rest is documented policy. Tagged pages also get
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
