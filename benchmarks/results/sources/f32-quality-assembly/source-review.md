# F32 interval source review

This review applies to the F32 candidate product digest
`4d6f7ab3f68500009aa938b1e3680d8c26a6fc052f706dd3a4ce5ad697b94a8e`
over retained commit `f9738174757252567730ff62b5d36620c01d1e6c`.

The change extends the existing N16 output interval to F32 through shared
`Storage<T>`. Span validation still precedes interval extension; source-color
conversion retains its previous position. Fragment tracking extends only at
validated row callbacks, retaining the conservative zero-coverage halo and any
successful rows before a later failure. Pixel arithmetic and quantization are
unchanged. Dense output still supports arbitrary destinations.

The three dynamic writers (color/pattern, direct shading, image) prepare the
full F32 interval once before their existing pixel loops. `paint_dynamic`
retains its format/index validation and arithmetic. Preparation does not
invalidate a wide solid-span cache; the actual mutable pixel access still does.
Preparation changes only metadata and performs no allocation or resource charge.
F32 eager initialization and the sixteen-byte pixel payload remain unchanged.

Page output may skip the conservative range exterior because existing admission
requires the same opaque background already present in the destination. Tests
cover colored backgrounds, distant stores and holes, partial errors, empty
spans, dense conversion, dynamic partial failures, irregular/offset tiles and
reused storage. F64 admission and each accuracy policy remain unchanged.

The completed full check and six-arm diagnostics are separate evidence. The
quality assembler verifies their raw logs and measured layouts. Fixture route
and independent full-output checks are also required separately. Source review
alone proves neither all-input correctness nor RSS neutrality or a speed gain.
No new dependencies, unsafe code or additional heap storage are introduced.
