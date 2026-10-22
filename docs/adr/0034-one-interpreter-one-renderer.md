# ADR 0034: One interpreter, many devices; one renderer

Status: accepted, 2026-10-21; fast parameters retired 2026-10-22 (§6).
Supersedes the archived ADR 0033 proposal
(branch `codex/renderer-n16-interval`, `docs/adr/0033-*`). Amends ADR 0004
(rewritten in the same change), ADR 0012 (the `Store`) and ADR 0016 (render
budgets), and replaces the crate table of `docs/ARCHITECTURE.md`.

## Context

Rendering grew six execution routes: a whole-page image route, a byte page in
256-row bands, chunked byte and f64 compositing, the exact general compositor
in 256×256 chunks, an 8-bit opaque route that rasterizes straight into the
output, and a sole-image route inside the last two. Storage precision was
chosen for the whole page, so one soft mask in a corner put every pixel on
56-byte f64 values. Every region re-ran every command, re-converting colours,
re-outlining strokes and re-deriving coverage; shadings evaluated the whole
function and colour chain per sample; images had four samplers. Budgets
borrowed fields with other meanings, errors surfaced in region order, and the
`Accuracy` mode selected different code from compilation onwards. Text
extraction paid for painting.

Every competing engine that is faster than Pagura gets its speed from
representation and scheduling (coverage only where edges touch, per-band
command lists, write-once output, cached decoded resources) as much as from
kernels; the ones that are also less accurate get the rest from 8-bit
arithmetic, which ADR 0004 rules out. Pagura has no users yet, so the
interfaces can change wholesale.

## Decision

### 1. Crates and data flow

```
pagura-content     tokenizer, typed operators (Annex A), source positions
pagura-interpret   graphics state machine → Device; Needs; SceneRecorder; TextDevice
pagura-color       colour spaces, functions, prepared converters; ICC and CIE
pagura-image       image streams decoded on request: reduction and region
pagura-raster      geometry, flattening, stroker, band rasterizer → coverage spans, clips, quantize_unit
pagura-paint       preparation and reservation, band executor, prepared paint, span kernels, storage formats
pagura-render      RenderContext, RenderOptions, orchestration
pagura-appearance  form-field and annotation appearance generation
pagura-core        values, errors, limits, Meter, Store
pagura-reference   test only: slow, obviously correct f64 raster and compositor
```

```
Document ─► pagura-interpret (Needs) ─► Device
                                          ├─ SceneRecorder ─► Scene (per page, device independent, in the Store)
                                          └─ TextDevice ─► pagura-extract (no image, shading or colour work)
Scene + RenderRequest ─► prepare (source order: cull, validate, reserve, coverage sources,
                         prepared paint, buffer formats, band bins) ─► PreparedScene
PreparedScene ─► band executor (infallible, cancellable between bands) ─► span kernels ─► quantize once ─► RGBA8
```

Dependencies point downward: `pagura-render` → `pagura-paint` →
`pagura-interpret` → `pagura-content`, `pagura-color`, `pagura-image`,
`pagura-raster`, `pagura-text`, `pagura-font`, `pagura-extract`,
`pagura-appearance`, `pagura-document`, `pagura-core`. `pagura-color` stays free of PDF objects
(ADR 0028): the interpreter resolves colour-space and function objects into
its types.

### 2. One interpreter, many devices

`pagura_interpret::Device` is public. The interpreter asks the device for its
`Needs` before it starts and does no work a device does not need: a device
without `paint` causes no colour-space resolution, shading or pattern parsing,
soft-mask interpretation or image decoding; without `outlines` no glyph
outline is loaded; without `text` no Unicode mapping is built. The pixel path
is monomorphized over the device; trait objects are for cold sinks only.

Fonts load the same way for every device need, mode and region. A font's
structure (header, table directory, and the tables that locate a glyph:
`cmap`, `loca` and `hmtx`, the CFF indexes and charset) is validated when the
font is first loaded; a glyph outline may be decoded and validated at its
first use while content is interpreted, and is then cached per font and
glyph. A malformed glyph that is drawn fails or is repaired exactly as an
eager decode would report it, in source order; a glyph that is never drawn
is never examined, so damage confined to unused glyphs is neither an error
nor a diagnostic. Errors therefore depend only on the glyphs a page draws,
never on mode, region or cache state.

Devices in this pass: `SceneRecorder` (records a `Scene`) and `TextDevice`
(feeds `pagura-extract`). SVG output, PDF re-emission and hit testing are
roadmap devices the trait is designed to admit.

### 3. The scene

A `Scene` is device independent and policy free: no accuracy, resolution or
`Limits` decision changes what is recorded, and fonts load the same way for
every rendering. Marks are stored in an arena in source order and addressed
by index; groups, soft masks and tiling-pattern cells hold their children as
index lists. Paints, composite states and clips are interned, so a colour is
converted once per paint and render, not per mark or pixel. Images are
recorded as undecoded sources; decoding is a paint-time request. Form
XObjects are recorded inline (their marks depend on inherited state); a form
group becomes a group. The scene reports its allocation size and is cached in
the `Store` under (document, page, optional-content configuration).

### 4. One renderer

`pagura_paint::prepare(&Scene, &RenderRequest, …)` is the only fallible step
of rendering. In source order it culls marks by conservative device bounds
against the requested region, validates them, reserves their work and bytes,
builds coverage sources and prepared paint for marks that meet the region,
chooses each buffer's storage format from its group's whole content, and bins
marks to bands.

The band executor runs bands of a fixed height on the global device grid,
independent of the requested region. Marks activate at their first band and
retire after their last, in source order. Groups and soft masks get
band-local buffers over their bounds; a soft mask's group runs for the band
before its members; knockout keeps per-member shape and alpha. A span fully
covered by a later opaque Normal mark of the same group, with no mask,
transfer or non-Normal blend between, may be skipped for earlier marks. Each
output pixel is quantized once with `pagura_raster::quantize_unit`, onto a
background premultiplied once.

The band rasterizer accumulates a mark's edges per band into a dense row of
f64 cells with a touched-cell bitmap and no sorting (ADR 0004 §4); cells
with conflicting windings go to the exact local resolver; glyph quadratics
are integrated against the true curve; interiors leave as full spans. For the same flattened geometry every exact producer yields the
same coverage bytes. Clips are prepared once per clip state and classified per
band row into outside, inside and boundary runs; exact joint geometry runs only
where mark and clip are both partial.

Prepared paint is a closed set chosen once per paint and render: solid,
axial, radial, function-based, mesh, image sampler, tiling cell. It fills a
span of working colours or reports a constant. Colour conversion is a
prepared converter; functions are total after preparation.

### 5. Precision and storage

Each buffer (page, group, soft mask) gets its format before its first store,
from its group's whole content and never from the requested region: N16 for
Normal source-over chains within ADR 0004's bound, F32 under its count bound,
F64 for every buffer that feeds an amplifying operation (including the
backdrop of a non-isolated group that contains one) and for buffers with spot
channels. Spot colourants under overprint simulation are extra channels of
the buffers, not replays.

### 6. Exact rendering

There is one pipeline and one rendering: every pixel within one level of its
exact result (ADR 0004). Fast parameters were retired on 2026-10-22 by owner
decision: Exact is the only rendering. A deliberately rough preview profile
for thumbnails and first paint is on the roadmap.

### 7. Failure

Interpretation fails as a whole, with no partial scene. Preparation runs in
source order, validates every input and reserves every work and byte budget
(ADR 0016: `render_bytes`, `function_work`, and one render-work meter);
the error it reports is the first in source order among the marks that meet
the region, independent of band height and partition. Execution cannot fail;
cancellation between bands is a stop, not an error. Type 4 calculator
programs are validated statically (stack effect and operand types per
branch); a program that cannot be validated fails in preparation and
arithmetic faults get defined results. Budgets are per render, so a whole page
can reach a limit that its tiles do not.

### 8. The Store

`pagura_core::Store` owns every reusable byte of rendering under one budget:
`get_or_build(key, build)` returns a shared value and evicts entries only the
store still holds. Keys in this pass: recorded scene, decoded image (source,
reduction, region), colour transform, font program, glyph edge setup,
prepared function and shading. Keys use resource identity, never addresses.
A prepared-scene key waits until a viewer benchmark shows reuse.

### 9. Determinism

Pixels, error kind, object and position, and charges are identical across
tile partitions and band heights for the same region, and native, WASI with
SIMD and scalar WASI produce identical bytes. Band buffers bound WebAssembly
memory growth. `PreparedScene` is `Sync` and bands are independent, so band
threads can follow; this pass is single threaded.

## Consequences

- The superseded routes are deleted: the whole-page image route, the byte
  page, chunked and banded compositing with the page-wide plan, the 8-bit
  opaque route (and with it ADR 0004's known defect), `lowp`, the narrow
  storage module, the duplicated image samplers, the fragment rasterizer, the
  strip sweep, certified accumulation, signed and approximate cells and
  rectangle code as separate backends, spot replays, the exact re-render and
  double compilation of the former fast mode, private quantizers, per-render
  clip and hull caches and the unbounded glyph hull map. The exact
  mathematics moves into the band rasterizer and into `pagura-reference`.
- Error order changes from region order to source order over the marks that
  meet the region; errors that depended on a page's tiling no longer do.
- Text extraction no longer decodes images, parses shadings or converts
  colours.
- `RenderOptions::accuracy` is removed: rendering is exact (§6).
- New devices (SVG, PDF re-emission, hit testing), native band threads and a
  prepared-scene cache are roadmap items (`docs/ROADMAP.md`).

## Alternatives considered

- **Keep both modes as routes and narrow precision route by route** (the
  experimental branch): slower than its own starting point on 49 of 83
  measured cells; rejected.
- **Per-16×16-chunk promotion from u8 to N16** instead of per-buffer formats:
  needs a proof of the bound for a u8-rounded start and chunk-aligned work for
  tile independence; deferred to the performance phase as an experiment.
- **Forms as shared sub-scenes:** their marks depend on inherited graphics
  state, so sharing needs a state-independence test per form; deferred.
- **A scene without interned paints and states:** simpler recording, but it
  moves colour conversion back to per mark; rejected.
