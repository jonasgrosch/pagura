# Stack integration and glyph resource correction

2026-10-21. The stack based on `bc2add8e` now includes the resource and stroke
corrections from `66e2aa33`, `29b9ee5b`, `ef7a5623` and `a2a71c3e`, with the glyph
budget strengthened after reproducing two native accounting defects and a WASI
overflow. Existing glyph memoization, rowwise JPEG accumulation and exact u16
output conversion remain. This is correctness/resource integration; no new speed,
RSS or competitor result is claimed.

The imported changes check page-pixel and byte arithmetic before allocation,
bound recycled surface capacity, include that storage in retained-byte reporting,
refuse byte-band writes before a window, stabilize near-reversal stroke joins and
butt-end admission, and exercise both rendering policies in the fuzz drivers.
The repository's stable fuzz replay compiles those same drivers; no new
coverage-guided fuzz campaign was run.

## Glyph findings and correction

A tall-then-short glyph sequence retained **1,328 bytes against a 1,128-byte
allowance** in the imported implementation. A separate two-fragment case accepted
an allowance one byte below its required row tables, fragments and complete
four-lane quad. Padding lanes consume storage even when they paint nothing.

The correction counts span lengths before per-row traversal, accounts for retained
row/fragment capacities and padded quads together, trims excess scratch capacity
only when needed, and verifies actual capacities after reservation. Admitted
geometry keeps its clipping and fragment order. Independent unit-square masks
check the resulting pixels.

Checked addition also prevents `cells + SPARE` overflowing on WASI32. The probe
constructs a private extent directly: it demonstrates the arithmetic defect, not
reachability from a malicious PDF under default parser limits.

The bound covers these vector payload capacities within half of `decoded_bytes`.
It is not a total-RSS bound, an allocator-overhead bound or a transient-allocation
guarantee. Earlier extent rejection does not promise to clear all warm scratch.

## Validation and retained failures

| Attempt | Result |
| --- | --- |
| full-check-001 | Canceled during fixture generation after policy passed; exit 130. Review had identified the incomplete glyph budget. No compilation or full-check pass. |
| stroke-check-001 | Seven focused stroke tests passed, including the 1,000 seeded piece/union comparisons. |
| reproducer-001 | Native suite: 10 passed and the two intended regressions failed. The helper then failed to parse interleaved test output; its report stays incomplete. |
| reproducer-002 | Reused the exact native failure log without rerunning it; corrected parsing confirmed both failures. The WASI extent probe reproduced the overflow trap. |
| fixed-001 | All 12 native and 12 WASI glyph tests passed before the later helper extraction. |
| clippy-001 | Rejected a 107-line constructor against the 100-line limit. Reservation moved into a helper; no lint suppression. |
| full-check-002 | Complete check passed on the frozen extracted implementation: 881 Rust tests, 5 doctests, 196 tooling tests, 1,249 conformance cases, host/native/WASI parity and strict documentation/lint checks. A subsequent release WASI build passed all 12 glyph tests. |

The final full-check receipt completed with unchanged source and no evidence
errors. Its observer made 168 scans and recorded no known competing jobs. This
cooperative desktop observation does not certify dedicated compute or detect
every short-lived process.

After that frozen check, four comment-only changes clarified the Competitive
fallback and corrected a stale test comment. A compositor limit retries the
same compiled list once with exact compositing; reduced image samples or
drawing-only font validation may remain, the retry may fail, and preprocessing
can add attempts. It is not an Exact-policy recompilation guarantee. Separate
formatting/documentation validation and before/after hashes bind those comments;
the completed full check is not relabeled as execution of a different source hash.

## Evidence

[Bindings](stack-resource-integration.json) authenticate the completed outer
receipts, frozen inputs, logs and archived binary/module hashes. The raw archive
is retained locally at `local/stack-resource-integration-archive-001/` (90 files,
38,873,382 bytes before the later documentation-only records). Executed helper
sources and original patches are retained under
`sources/stack-resource-integration/`.

Some historical helper reports hash their own receipt while it was still running.
Those hashes are explicitly nonauthoritative snapshots. The finalized outer
receipts bind the reports; original reports are preserved without rewriting their
claims. Local helpers were outside the runner's source digest: retrospective
copies preserve their retained bytes but do not independently prove execution-time
bytes. This evidence assumes the trusted local coordinator and runner.

All failed and canceled attempts remain visible. These finite checks establish
neither arbitrary-PDF correctness nor the independent worldwide performance and
accuracy goal. The next measurements need a fresh baseline that includes this
integration; historical speed ratios must not be multiplied together.
