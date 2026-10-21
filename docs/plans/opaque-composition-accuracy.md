# Close the plain opaque composition gap

Status: next independent accuracy investigation; static review on the glyph-memo
candidate, 2026-10-21. No new renderer measurements or fix are claimed here.

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

The current opaque kernel analytically predicts 217: its initial source is
217, and each partial store is round(242*10/255) + round(217*245/255) = 217.
This is a prediction to reproduce, not a recorded render. Under the one-level
contract the admissible bytes are 218 and 219; tolerance 1 against rounded
oracle 219 would incorrectly also admit 220.

One thousand black marks of coverage byte 1 over white have exact result
255*(254/255)^1000 = 5.012754... and admissible bytes 5 and 6. Byte storage can
stagnate at 127 even with one combined rounding per store. Fixing the double
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
