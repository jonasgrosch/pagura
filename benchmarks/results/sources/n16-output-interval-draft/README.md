This directory archives the unapplied patch and base hashes. The original source-only review package, including base and overlay directories, remains at `/private/tmp/pagura-n16-interval-current`. No measurement is associated with this draft.

The [validation checklist](VALIDATION-CHECKLIST.md) names the existing native and
WASI checks, layout probes and fixed performance controls. The separate
[harness draft](harness/README.md) has been reconciled against the current shared
helpers without replacing them. Its interruption handling received source review;
all proposed tests and benchmarks remain unexecuted. Neither draft has been
applied in this frozen measurement worktree.

Source preparation now also exists in the isolated `codex/renderer-n16-interval`
worktree, based on `a9814ee4`. The reviewed product and harness patches are
applied there but remain unvalidated. This frozen measurement worktree and its
binary/input bindings remain unchanged. Execution and promotion of the candidate
follow the completed start/current comparison and available local compute.

The standing retention policy still distinguishes hard correctness/resource
failures from complete performance admission. A useful measured partial gain
may remain experimental without claiming all latency and RSS gates passed.
This draft changes no existing gate or reference.

# N16 page-output interval — current-stack draft

Unapplied, unformatted and unexecuted source-only proposal. No active worktree
was edited; no build, test, import, render, profile or benchmark was run.

Base: `/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura`,
commit `a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c`.
`base/` preserves the four existing touched files; `overlay/` has the proposed
versions and one new public test module. `base-sha256.txt` binds the copied
inputs. `painted-interval.patch` is the combined patch for coordinator review.
The currently running start/current comparison must finish and be audited
before any application or product measurement.

## Product scope

Only `compositor/narrow.rs` and `compositor/output.rs` change product behavior.
N16 keeps its same `Vec<[u16;4]>`, initialization, source encoding, full-span
fills, fractional stores and dense output converter. `Words` adds one flat
conservative interval of submitted stores. No mark scan, source cache, geometry
change, precision promotion, optional fallback or new heap allocation is added.

The two N16 writers maintain it:

1. `Buffer::path` first encodes the same source and checks the same pixel slice.
   A nonempty valid slice then widens the interval before the unchanged store.
   Invalid/empty ranges preserve their previous errors or no-op behavior.
2. `MaskSink::N16` uses the existing shared `mask_rows` preparation. Its callback
   receives the validated surface row/column only after that row's mask lookup
   and clip product/rectangle evaluation. It widens the interval before the
   unchanged fill/mix. A later failing row leaves earlier successful stores and
   their interval intact. Fully excluded masks do not widen it.

The shared row callback gains a surface-position argument; only N16 consumes
it. F32 ignores it and retains its existing arithmetic, validation, clipping and
full-run fill. A row-index iterator supplies the argument; optimization should
remove unused position tracking from F32, but this is not a generated-code or
performance claim. The generic helper remains single-source. U8 and all F64
product code are unchanged.

Page output adds one format/range query per narrow chunk; U8/F32 then run the
same dense loop. That fixed dispatch cost is a protected measurement concern,
not a claim of zero overhead on the other narrow formats.

`Context::write_output` skips only N16 pixels outside the interval. The existing
render-policy guard selects F64 for nonopaque page backgrounds. The output
Pixmap already contains the same opaque constructor background before disjoint
chunks execute; unsubmitted pixels therefore already have their required bytes.
The interval includes all holes between its endpoints. Dense `Buffer::output`
still writes every requested complete pixel even when the interval is empty.

No source validation, function-work ledger, raster work, resource admission,
group traversal, chunk boundary or tile-dependent precision decision changes.
N16 allocated payload remains pixel_count*8, and the existing `bytes()` ledger
is unchanged. This does not remove N16 initialization or output initialization.

## Focused tests in the patch

- Existing exhaustive word converter and arbitrary-destination dense-output
  test now deliberately uses an empty painted interval.
- Path stores: untouched, empty, invalid bounds, zero coverage, full coverage,
  repainting background, separated writes and unchanged byte ledger.
- Fragment stores: negative/nonzero origin, a valid first product row followed
  by a failing row, fully excluded masks, and two corner writes spanning a
  complete interval while independently checking every background hole.
- Existing product-then-fractional-clip test checks N16 metadata and confirms
  U8/F32 expose no interval. Existing F32 bit-identity tests are unchanged.
- Output tests check empty and painted colored-background chunks, nonzero x/y
  origins, different source/output stride, two neighboring chunks, partial
  coverage with independent [8,42,85,255] bytes, a multirow interval with holes,
  and unchanged context work/allocation counters. U8/F32 are dense controls.
- New public tests use literal integer PDF rectangles and independent device
  occupancy. Both modes cover full pages, a complete irregular 1/255/256/257
  partition in both dimensions, offset tiles crossing x/y=256 and512, blank
  chunks and the final one-pixel chunk. A painted→blank→painted RenderContext
  sequence checks changed opaque backgrounds and actual Competitive spare
  pointer reuse. These public rectangles exercise path coverage; the direct
  fragment tests separately cover the Competitive fragment writer.

## Coordinator-only verification and layout gate

First format/compile and run the focused tests plus existing N16/F32 masks,
independent opaque accumulation/colored nonopaque-background tests, off-region
source errors and work/decoded-byte boundary tests. Run the corresponding
focused native, WASI SIMD and WASI scalar checks. No test result is claimed here.

Before and after the patch, use the same temporary internal diagnostic test to
print `size_of` and `align_of` for Buffer, Option<Buffer>, Surface, Context and
MaskSink, under the exact native and wasm32-wasip1 toolchains/configurations.
The new payload adds two usize fields; a conventional layout would make Words
40 bytes on64-bit and20 bytes on32-bit, versus Vec's24/12. These are unmeasured
layout hypotheses. Rust enum layout and enclosing types must be measured;
do not infer their sizes from source order or the currently largest variant.
Reject or explicitly redesign if enclosing layout grows under the frozen
experiment's no-growth requirement. Do not silently exempt fixed stack bytes.

Review the optimized F32 mask-loop body if unchanged dispatch cost is needed;
the unused callback position should vanish, but only generated code or controls
can establish that. No product layout reporter is added by this patch.

The existing fixed N16 experiment already provides the required empty, distant,
dense, tiny, F32/F64, image and tile controls. Use a fresh baseline and candidate
bound to the retained current stack, without mixing old measurements. Distant
corner marks deliberately retain a full flat interval and should receive no
scan-saving claim. Reject on pixel/error/work differences, layout growth, or
failure of the separately frozen latency/memory decision policy. No speedup,
RSS saving or admission is asserted by this draft.
