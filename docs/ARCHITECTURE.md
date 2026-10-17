# Architecture

## Crates and dependency direction

```
pagura-host (WASI command, ADR 0005)      pagura (facade + CLI)
        └──────────────► pagura ◄────────────┘
                           │
        ┌──────────────────┼───────────────────┬──────────────┐
        ▼                  ▼                   ▼              ▼
  pagura-render ──► pagura-raster        pagura-write   (re-exports)
        │                  │                   │
        ▼                  │                   │
  pagura-document ──► pagura-codec             │
        │       └───► pagura-crypto            │
        ▼                  ▼                   ▼
                     pagura-core ◄─────────────┘
```

Dependencies point downward only and are fixed in the crate manifests.

| Crate | Responsibility | Knows nothing about |
|---|---|---|
| `pagura-core` | `Value`, `Name`, `Dictionary`, `ObjectId`, `Rect`; bounded `syntax::Parser`; `Error`/`ErrorKind`/`Feature`/`Position`; `Diagnostic`/`Repair`/`Recovery`; `Limits` | files, revisions, rendering |
| `pagura-codec` | stream filters and predictors with streaming output caps | documents, references |
| `pagura-crypto` | MD5, SHA-2, RC4, AES; the standard security handler from plain parameters | the object model beyond `ObjectId` |
| `pagura-document` | immutable source bytes, cross-reference data and revisions, lazily parsed and cached objects, decoded streams, typed page tree, recovery | content streams, pixels |
| `pagura-raster` | geometry, paths, flattening, exact-area fill, stroking, masks, `Pixmap` | PDF |
| `pagura-render` | content interpretation into a display list in PDF terms; page-to-device transform; tiles; painting through `pagura-raster` | file structure |
| `pagura-write` | deterministic serialization | parsing |
| `pagura` | the public facade and the `pagura` CLI | — |
| `pagura-host` | the stdio session protocol, native and `wasm32-wasip1` | — |
| `pagura-testkit` | test-only fixture access and assertions (std only) | product code |

Add a crate only for a substantial, independently testable responsibility
with a clear dependency direction (fonts, images and colour will each get one
in M3–M5).

## Invariants

1. Original bytes are immutable. A no-op save can return the original bytes.
2. Object references are document-relative, generation-aware identities. A
   reference to an undefined object resolves to null (clause 7.3.10).
3. Every traversal has an explicit budget; every decoder enforces an output
   cap while decoding.
4. A rendering error cannot become a successful partial page.
5. Unsupported behaviour is an `Unsupported(Feature)` error with a stable tag;
   repairs are `Diagnostic`s with stable `Repair` tags (ADR 0003).
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
composition targets premultiplied RGBA8 tiles. A deterministic CPU renderer is
the diagnostic reference, not an infallible oracle.

## Editing (M7)

Editing uses immutable snapshots and transactions with copy-on-write resources,
explicit undo/redo, and dependent cache invalidation. Incremental save, full
save, signature-aware save and sanitizing redaction have separate contracts. A
rendered scene is never the sole editing representation. Forms and tags must
survive edits. A repaired document is saved only by full rewrite.

## Performance policy

First remove redundant parsing, allocation and copying, then improve data layout,
caching, scheduling and algorithms. Profile before optimizing. Keep algorithmic
fallbacks explicit and bounded. Do not trade fidelity, security, portability or
the no-unsafe rule for speed. WebAssembly builds use SIMD128 through
autovectorization; architecture-specific native builds are a separately labeled
benchmark track with identical competitor tuning.
