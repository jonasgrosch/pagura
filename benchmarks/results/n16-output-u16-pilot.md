# Exact 16-bit N16 converter: native pilot

2026-10-21. Pilot only; no confirmation or full performance admission.
The candidate uses `word.saturating_add(128)` in the existing shift/subtract
quantizer. For words through 65,407 the sum is unchanged; above that boundary,
both the original rational round and saturated expression return 255.
Exhaustive u16 tests, nonzero source offset, incomplete-pixel tails and excess
destination capacity pass. This removes the former
lint exception and adds no buffer or retained state.

Against committed `f98ad836`, the five-pair native pilot verified 920 outputs.
Exact opaque64/full median ratio is 0.8788 (12.1% lower, upper 0.9274), and
Competitive is 0.9139 (8.6% lower, upper 0.9693). Only Exact passes the full
target latency gate. Across the matrix, 13/44 protected latency and 34/46
peak-RSS gates pass. Both target RSS gates pass in this pilot. Small-sample
tail-regression signals remain for Competitive opaque-clip and Exact
opaque-tiny cold; the complete ledger preserves every cell and bound.

ARM disassembly of the pinned binary uses 16-bit saturating vector addition,
shifts and narrowing subtraction. That establishes the intended lowering,
not a universal speed gain. Scalar WASM may pay for saturation, so a separate
fixed diagnostic must precede native confirmation. No samples will be pooled
with the previous experiment or a later confirmation.

Focused tests, renderer Clippy, native/WASM builds, eight independent precision
outputs and default Exact native/WASI host parity pass. The first build stopped
on an obsolete lint expectation; the corrected build passed. Full integrated
repository validation remains required before retention.

[Complete pilot audit and evidence hashes](n16-output-u16-pilot.json),
[frozen native plan](n16-output-u16-plan.json),
[source/binary bindings](n16-output-u16-bindings.json).
