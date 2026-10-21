# Close the plain opaque composition gap

Status: independent baseline reproduced on the glyph-memo stack, 2026-10-21.
The [diagnostic evidence](../../benchmarks/results/opaque-precision-baseline.md)
confirms both Exact failures below and the long-stack failure in Competitive
against the one-level target. No correction or performance gain is claimed.

Exact's eligible plain opaque pages still reach `DisplayList::render_opaque`.
Its eligibility predicate proves source opacity, not full pixel coverage.
`PathCommand::rasterize` converts source colours to RGBA8 before painting, and
`pagura-raster/src/raster.rs::composite` rounds source and backdrop products
separately. ADR 0004's general-composition amendment explicitly leaves this
route unchanged. The current precision fixtures wrap opaque marks in a
knockout Form, so their success does not close this gap.

## Independent reproducers

Add plain-root siblings of `content/precision-two-partial` and
`content/precision-slivers-1000`, linked to `content-compositor-precision`.
Retain the existing fixtures and strict expectations. Confirm the compiled
commands and selected route; matching pixels alone cannot establish coverage
of the opaque shortcut.

For a 1-by-1 page, opaque gray 0.85 followed by two gray 0.95 rectangles of
coverage byte 10 gives the existing policy's exact composition in byte units:

    255 * [0.95 + (0.85 - 0.95) * (245/255)^2]
      = 44617/204 = 218.7107843137...

The current opaque kernel returns 217, matching the analytic prediction: its initial source is
217, and each partial store is round(242*10/255) + round(217*245/255) = 217.
This is now a recorded render. Under the one-level
contract the admissible bytes are 218 and 219; tolerance 1 against rounded
oracle 219 would incorrectly also admit 220.

One thousand black marks of coverage byte 1 over white have exact result
255*(254/255)^1000 = 5.012754... and admissible bytes 5 and 6. Byte storage can
stagnate at 127 even with one combined rounding per store; both modes returned
127 in the direct-root probe. Fixing the double
round alone therefore cannot establish the contract. Keep unrounded analytic
intervals in focused tests in addition to existing PPM expectations.

## Implementation and acceptance

1. Reproduce both direct-root cases on the committed stack, including opaque
   alpha and full/tile equivalence. Record both modes separately. Do not invent
   a Competitive allowance to hide a newly exposed failure.
2. Evaluate reuse of existing bounded N16/general composition for Exact opaque
   scenes lacking a full-coverage proof. Preserve a byte shortcut only where
   its complete error bound holds. Avoid a second independent wide compositor.
3. Check existing pixel-aligned cases, long overlaps, source quantization,
   transfer and group exclusions, error positions, work limits and native/WASM
   equivalence. Any changed resource behavior must be explicit, not silently
   removed from the matrix.
4. Freeze a separate incremental speed/RSS experiment on opaque paths and text,
   including ordinary workloads and disabled controls. Keep the glyph memo
   identical on both sides. Report the accuracy gain and any speed/memory cost
   separately; do not pool samples with the memo or clip experiments.

Reject a proposed fix if it only moves the failure to longer overlap stacks,
uses baseline output as truth, weakens the independent contract, or requires
unbounded storage/work. An accuracy correction may have a measured cost; it
must not be presented as a performance gain. The standing goal requires both
contracts to be met before broad superiority claims.

## Preferred implementation seam after baseline reproduction

Reuse general composition and its bounded N16 page storage. Remove the legacy
traversal only after shared source validation prevents culling from suppressing
required errors. A source-order N16 sink in the old traversal is a compatibility
alternative if supported completion blocks that migration; it reuses
`Rasterizer::clipped_path`/`clipped_glyph` and the existing narrow kernel, but a
single-pass full-tile buffer costs eight bytes per pixel plus final output.
Keeping that second traversal indefinitely would obstruct the unified design.

Static review found a concrete migration risk: ICC forward transforms can
parse successfully as unsupported and fail only when evaluated. The legacy
opaque route evaluates source color before rasterization; general composition
can cull an off-region path or entire group before conversion. Add direct-root,
nested-group and off-tile siblings of the existing unsupported ICC fixtures,
plus a parsed Type 4 tint function that fails when evaluated. Require the
feature/object/source context rather than accepting a blank successful page.
These off-region cases are proposed regressions, not yet executed evidence.

The planner can host bounded validation, but its early precision-exclusion
returns must not skip remaining required source checks. A validation-only
prototype duplicates conversion work; measure and label that cost. Later
prepared-color reuse must include model/intent/context identity and bounded
storage in the shared scene index. The current 64 root hull slots are not an
all-scene color cache. Nonlinear device transfers remain deferred device
operations; do not bake them into resolved source colors.

General routing changes planning and aggregate work charges, transient storage,
and potentially first-error selection. Test and document these changes; do not
raise limits, reset work counters on retry or fall back to inaccurate byte
composition. Existing Exact chunks already cap N16 pixel scratch at 512 KiB;
smaller regions trade scratch for replay work. Both existing output constructors
use the pixel limit, so this is not a new decoded-byte cap on final output.

For Competitive, retain an actual N16 plan instead of forcing it to U8, while
preserving other storage exclusions. Keep fragment construction, cache lookup,
clipping, placement order and work charges. Extend only the narrow mask sink
to consume raw floating source colors with shared N16 arithmetic. Fragment
rows may mix 255/partial/0 coverage; `Buffer::path` assumes homogeneous full
spans and cannot accept those rows unchanged. Keep N16 chunks bounded and test
mixed rows, fractional clips, nonzero origins, cold/warm caches and work-error
boundaries before measuring the speed and memory cost.
