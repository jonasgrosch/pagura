# Consolidated architecture direction

Status: proposal for decision, 2026-10-21. Documentation only. Nothing in it
starts until the decisions in §5 are taken.

This document merges two reviews written on the same day into one direction:

- **The architecture review** ("Pagura Architecture Review", a private page,
  21 October 2026): every competing engine compared with Pagura on 18
  architectural dimensions, a target architecture for the whole engine, a
  six-phase programme and nine decisions.
- **The ADR 0033 challenge** ([adr-0033-challenge.md](adr-0033-challenge.md)):
  a skeptical review of the proposed prepared renderer against main, real
  documents, a focused profile and competitor sources, with a renderer
  architecture, gates, experiments and migration.

The architecture review already builds its renderer phase on the challenge.
This document resolves what still differs, adds evidence neither used, and
states one sequence and one decision list.

## 1. Agreed direction

Both reviews independently reach the following, and this document adopts it.

- **Foundations stay fixed:** no `unsafe`, no third-party crates, typed
  budgets charged before allocation, reported repairs, explicit `Unsupported`
  tags, deterministic output, the one-level accuracy contract.
- **One renderer.** Marks are prepared once per render; execution runs in
  bands over prepared marks in source order; coverage exists only where edges
  touch and interiors are spans; clip intersection is resolved exactly only
  where mark and clip are both partial; opaque work hidden by later opaque
  spans is culled exactly; span kernels are generic over format and blend
  class; each output pixel is quantized once.
- **Order inside the renderer:** coverage first, prepared paint second, the
  executor third.
- **Failure:** preparation validates and reserves every budget in source order;
  execution cannot fail. Cancellation between bands (a viewer requirement from
  the architecture review) is a stop, not an error.
- **One pipeline under the contract, eventually.** Exact and Competitive
  collapse into one pipeline with a few explicit fast parameters, but only
  after the new renderer passes its port gate. Competitive stays until then.
- **Freeze new admission-gated kernels** until the one renderer exists.
- **Retire** the 8-bit opaque route, `lowp`, the page-wide N16/F32/F64 split,
  the duplicated image routes (`direct_image`, `opaque_image`,
  `image_interiors`), the separate coverage backends as backends, spot
  replays and the renderer's private quantizers, as their replacements land.
- **Elsewhere in the engine:** text extraction must not pay for painting;
  decoded images and colour transforms are reused across pages under one
  memory budget; ADR 0016 budgets are fully in force.
- **Not to do:** GPU rasterization, 8-bit numerics, relaxed SIMD or fused
  multiply-add in exact kernels.

## 2. Evidence neither review used

Three proof prototypes from 2026-10-19 (built on main `99bfb290`, not in the
repository) already tested parts of this renderer:

| Prototype | Design | Accuracy | Speed against hayro |
|---|---|---|---|
| VC, round 6 | Band cell accumulation with sparse retained cells, clip products and byte row kernels, kept within the accuracy gate; geometric union for strokes | Passes every gate (vector MAE 0.013 at 72 dpi, P99.9 1–2) | Vector 1.6–1.9×, mixed 2.2–2.4× cold; raster and composition are 80–97% of its time and must get 2–5× faster to reach 0.90× |
| VS | vello-style strips and tiles | At 150 dpi, MAE 3–8× hayro's (vector 1.66 against 0.21); maximum errors 255 | 0.79× at worst |
| CX | Low-precision certified compositor, f32 coverage, glyph mask caches | MAE 7.5 against hayro's 0.9 (mixed, 150 dpi) | ≤ 0.80× in all 24 cells |

What this changes:

- **"The gap is architectural, not the price of exactness" is a hypothesis
  with partial support.** The prototypes that reached hayro's speed bought it
  with accuracy. The exact one, VC, removed much of the scheduling overhead and
  still missed by 2–5× in raster and composition.
- **VC's remaining costs are the challenge's first two levers:** cell and union
  coverage plus stroke and bounds work per mark (E1), and shading sampling
  with source conversion (E2), plus image and mask footprints (M4). The
  challenge's profile of main agrees: `0000638` (about 10,500 marks, mostly
  two-segment strokes) spends about two thirds of its samples in exact
  coverage generation.
- **VC already rejected** bulk cell-edge accumulation, taller bands and
  quantized glyph caches. The proof work must start from its retained pieces
  and avoid repeating its rejected variants.
- **Strokes deserve their own experiment.** VC's geometric stroke union made
  the dense stroke page more expensive (1.73 to 2.92 billion instructions at
  150 dpi). The Competitive path already decomposes strokes into pieces that
  meet without overlapping, so their areas add without a union. Under exact
  integer accumulation that decomposition would make stroke coverage exact
  without union resolution wherever its preconditions hold (E1b below).
- **The component evidence supports one band rasterizer.** In Blend2D's
  benchmark data, a dense cell row per band with a bitmap of touched cells and
  no sorting leads on many small marks; vello's per-line tiles with a global
  sort are 2–4× behind there. Main's exact sweep spends about 11% of
  `0000638`'s samples sorting edges.

Consequence: commit to the destination, but make the decision to replace
Competitive and the scale of the port depend on E1, E1b and E2, measured on
the scoreboard harness.

## 3. Differences and their resolution

| Topic | Architecture review | ADR 0033 challenge | Resolution |
|---|---|---|---|
| Scope | Whole engine: interpretation, loading, memory, host, editor | Renderer only | The review's programme is the umbrella; the challenge's renderer plan is the inside of its Phase 2 |
| When renderer work starts | Phase 0, then a byte-identical seams phase (Device trait, crate cuts, policy out of compilation); proof work in parallel from Phase 0; port after Phase 1 | Contracts, then experiments E1 and E2 on main, then milestones on main | The proof work starts now, in parallel with Phase 0 and Phase 1, as the owner's breakthrough-first rule requires. E1 and E2 build components in their final homes (coverage rows in `pagura-raster`, prepared functions and colour in `pagura-color`), so a byte-identical success may merge early. Everything else ports after Phase 1 |
| Rasterizer | One band rasterizer (dense band cells, touched bitmap, certified integer accumulation, no sort) replacing the coverage backends | Coverage rows as the single raster-to-compositor product, first over the existing backends | Both: coverage rows are the interface; the band cell rasterizer is its first new producer (E1). For the same flattened geometry an exact producer gives identical bytes, so backends retire as it covers them |
| Storage precision | Per 16×16 chunk: start in u8, promote to N16 by a per-pixel partial-store counter; f64 per group for amplifying feeds | Format per buffer before its first store (ADR 0004's feed rule) in band-local storage | Start with formats per buffer: their bounds are proven and they are tile-independent by construction. Chunk promotion becomes a proof-work candidate, kept only if E4 shows N16 bandwidth matters. It carries two proof obligations: the bound when a value already rounded to u8 (up to half a level off) is promoted, which leaves almost no margin below one level after final rounding; and processing aligned to the global chunk grid, so that tiles cutting a chunk give the same bytes |
| 8-bit opaque defect | Phase 0 routes those pages to N16 until Phase 2 | Decide in E4 | Do not route them through the current chunked compositor. The experimental branch measured opaque text about 2× slower that way, and the worst real vector page (`0000638`, 132 ms on that route today) takes it. Either keep the defect documented until the band executor, or fix it in place with the counter-free 8.8 fixed-point store already proven in vector round 17 (3–11% on stress pages; its memory cost was for page-sized storage) |
| Crates | New `pagura-content`, `pagura-interpret`, `pagura-paint` | No new crate for the migration | Crate cuts belong to Phase 1 under the crate rule in `docs/ARCHITECTURE.md`. `pagura-paint` (prepared scene, bands, kernels) qualifies as a substantial, independently testable responsibility; the renderer plan does not depend on the cut |
| Caches | One `Store`, including a prepared scene keyed by page and scale | Two preparation scopes; a retained prepared page deferred until measured | One `Store` as the owner of every reusable byte; the prepared-scene key enters only when the viewer track shows reuse |
| Port gate | Within one level of the reference on the full corpus and stress set; no slower than Competitive in every cell on x86, Graviton and M4; WebAssembly parity | Per-slice decision rules for experiments; scoreboard milestone gates | The review's gate decides retiring Competitive; the challenge's rules decide proof work. The M4 runner is not available (its 24-hour minimum was declined), so the gate runs on x86 and Graviton plus the local M3 Pro unless M4 is provisioned |
| Open measurements | Viewer benchmark; browser-configured pdf.js | Memory attribution (peak RSS 1.7–2.2× hayro at 150/300 dpi), executor cost on opaque pages | Both belong in Phase 0 or the proof work |

## 4. One sequence

| Step | Content | Runs | Gate |
|---|---|---|---|
| Phase 0, ground truth | One rewritten ADR 0004; documentation drift fixed; a cross-path differential test of every fixture through every storage path and kernel; display-list snapshot tests; ADR 0016 budgets and `Meter` in render; `quantize_unit` in render; the premultiplied background and glyph row-budget fixes with their tests; partition tests for errors and charges; a viewer benchmark | Now | Output unchanged except the background fix |
| Proof work | E1 band cell coverage; E1b non-overlapping stroke pieces under exact accumulation; E2 prepared paint for every function type; E3 memory attribution; E4 band executor prototype with per-buffer formats against R4 and against chunk promotion | Now, in parallel with Phase 0 and Phase 1; competing variants in parallel worktrees; timed on the scoreboard harness | The challenge's decision rules (§10.4) and E1b's rule below |
| Checkpoint | Decide whether the one renderer is likely to meet Competitive's speed while exact, and how Competitive retires | After the proof work reports | Owner decision |
| Phase 1, seams | `Device` and `Needs`; policy out of compilation; scene fidelity; crate cuts; text as a device | Parallel with the proof work; small byte-identical PRs | Both modes byte-identical on the 120 cells and every fixture |
| Phase 2, one renderer | Port the proven design behind the port gate; delete superseded routes as each part lands; Competitive becomes explicit fast parameters | After the checkpoint and Phase 1 | The review's port gate (adjusted runners) |
| Phases 3–5 | Loading and memory, host and viewer, editor, as in the architecture review | Unchanged by this document | As in the architecture review |

**E1b.** Decompose strokes whose preconditions hold into pieces that meet
without overlapping and accumulate them in exact integer cells, falling back
to geometric union otherwise. Byte-identical to main's exact output, or
within ADR 0004's rules for an output-changing optimization with every
changed pixel reported. Keep it if `0000638`'s Exact time falls by at least a
third with no regression on the stress set.

## 5. Decisions

| # | Decision | Recommendation |
|---|---|---|
| 1 | Adopt the architecture review's programme as the umbrella, with the challenge's renderer plan as the inside of Phase 2 | Yes |
| 2 | Freeze new admission-gated kernels; byte-identical structural experiments (E1, E1b, E2, E4) are not such kernels | Yes |
| 3 | Run the proof work now, in parallel with Phase 0 and Phase 1, not after Phase 1 | Yes |
| 4 | Failure contract: validate and reserve during preparation; execution infallible except cancellation between bands | Yes |
| 5 | 8-bit opaque defect until the band executor: keep it documented, or fix it in place with the 8.8 store; not by routing through the chunked compositor | Keep documented until E4 reports; the 8.8 store if it must close now |
| 6 | Storage: formats per buffer first; chunk promotion only through proof work with its two proof obligations | Yes |
| 7 | Collapse Exact and Competitive into one pipeline with explicit fast parameters, effective only when the port gate passes | Yes in principle; decide at the checkpoint |
| 8 | Port-gate runners: x86 and Graviton, plus M4 only if provisioned | Yes |
| 9 | Governance: ADR 0004 rewritten once; ADR 0033 amended as the challenge's §13 lists, or superseded by one renderer ADR after the checkpoint; ADR numbers checked against main and unmerged branches | Yes |

The architecture review's other decisions are unaffected by this
consolidation and stand as it recommends them: shared object handles with
store eviction (ADR 0012 amendment), a JSPI transport as the browser baseline
(ADR 0005 amendment), deferring a host-adapter C ABI, deciding signature
creation at Phase 5, and a native AcroForm AF library instead of a
JavaScript engine.

## 6. What changes in the two source documents

- **ADR 0033 challenge:** its migration plan (§11) is replaced by §4 here; E1
  becomes the band cell rasterizer producing coverage rows; E1b is added;
  chunk promotion is recorded as a storage candidate behind E4.
- **Architecture review:** the Phase 0 item that routes 8-bit opaque pages to
  N16 is replaced by decision 5; the Phase 2 port gate runs on the available
  runners; the proof prototypes of §2 are added as evidence; "the gap is
  architectural, not the price of exactness" is stated as the hypothesis the
  proof work tests.
