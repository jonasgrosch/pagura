# Exact N16 output conversion: pilot retained, confirmation stopped

2026-10-21. The flat shift/subtract converter is algebraically identical to
rounded division by 257. All 65,536 word values and offset/partial/excess-output
slices passed focused tests; renderer clippy and the release builds passed.
There is no new cache, allocation or pixel representation.

The frozen 5-pair pilot verified 920 outputs against independent pixels and
same-mode baseline bytes. Both opaque64/full target latency gates and their
sampled RSS gates passed. Exact median ratio was 0.8944 (upper 0.9062),
Competitive 0.8720 (upper 0.9038). Across all 46 cells,13/44 protected latency
gates and 33/46 memory gates passed. These are exploratory results, not a
confirmation or a net speed gain against the earlier legacy byte route.

The 200-pair confirmation was canceled after six complete cells because a
separate code review found that shared narrow composition embeds straight
background channels where premultiplied channels are required for colored,
nonopaque backgrounds. This is a new independent accuracy blocker, not a
stop based on confirmation timings or memory. No partial confirmation
samples will be reused. Its one permitted launch is consumed. Preserve the
pilot and arithmetic patch, reproduce and fix background admission, then
freeze a revised candidate and fresh measurement protocol.

The [complete pilot audit](n16-output-pilot.json) keeps every cell and the
cancellation/provenance hashes. The original [plan](n16-output-plan.json) and
[bindings](n16-output-bindings.json) remain unchanged. The stopped report is
incomplete and cannot provide performance admission.
