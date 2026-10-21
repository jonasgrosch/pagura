# Clip cleanup on the combined performance stack

Status: **partial median win retained in the experimental stack, without full
performance admission**. The repaired fixed confirmation completed and the full
combined-candidate check passed.

The control is `202bda8d79481e467951b57ceb780a803dde4a37` (`combine/images`).
It includes the newer byte composition, four-entry clip cache, JPEG work, glyph
mask/charging changes, Competitive stroke pieces, and retained image interiors.
The candidate adds only conditional viewport construction in `Context::clip`.
The four-entry cache and every other product source are unchanged from the control.

This follows the user's direction to stack useful partial gains. Earlier evidence
and the restored-source full check are committed at `db957c32` on
`codex/unified-renderer-plan`; those checks do not validate this combined candidate.
The viewport idea is promising and unadmitted, not retired. The separate glyph
memo remains recoverable from `9ebac1cb`; its staged port is not part of this run.

The two checkouts contain identical independent-reference/artifact/runner tooling
and the same public-API benchmark example. Both release builds passed serially.
The pilot verified 400 outputs across 20 cells using exact independent RGB, alpha
255, and same-mode byte equality. A foreign renderer test was observed during it.

The [fresh plan](stacked-clip-plan.json) and [bindings](stacked-clip-bindings.json)
were frozen before measurements. The fixed confirmation uses 200 fresh pairs per
cell after five warmup pairs, with retained outputs and source/binary identities.
Historical samples and pilot samples are never pooled. All original numeric
thresholds remain in place. A secondary useful-gain/neutral-other-mode
classification guides further work only; it does not change admission.

This is the incremental current-stack pair. It cannot establish the total gain
relative to the old stack without fresh equivalent-work comparisons. It does not
measure a competitor, a representative corpus, or WebAssembly performance.
The [machine-readable record](stacked-clip-experiment.json) records results and
full candidate validation as they complete. No global accuracy or speed claim is made.

The first confirmation stopped after eleven completed cells: a process-group
probe returned EPERM after the harness had reaped its leader, and exception
cleanup repeated the error. This is an infrastructure failure. Partial timings
remain unanalysed and cannot be pooled into any future run. The repair preserves
the unreaped process-group leader through cleanup; treating permission denial
as absence would not prove descendants were drained. The separately frozen
[repair protocol](stacked-clip-repair-plan.json) and
[new bindings](stacked-clip-repair-bindings.json) authorized one fresh replacement.

## Completed replacement confirmation

The harness ran both unchanged binaries with the same repaired lifecycle helper.
All 8,200 outputs passed
independent exact RGB, opaque alpha and same-mode byte equality. No prohibited
compute jobs were observed during the 482-second run; this is not an attestation
of all background load, thermal state or a controlled release runner.

| Clip-churn full render | Control median | Candidate median | Median reduction | Median ratio upper bound | p95 ratio upper bound | Control / candidate peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Exact | 5.335 ms | 4.624 ms | 13.3% | 0.8799 | 1.0924 | 14.672 / 14.641 MiB |
| Competitive | 8.626 ms | 7.892 ms | 8.5% | 0.9309 | 1.2722 | 10.797 / 10.875 MiB |

Both target median bounds clear the 5% gain threshold. Neither target clears the
complete latency gate because its p95 upper bound exceeds 1.00. Six of eighteen
protected cells clear both latency bounds; eleven of twenty cells clear the raw
peak-RSS rule. Point p95 ratios are 0.9471 (Exact) and 0.9744 (Competitive); the
wide upper bounds do not establish a causal tail regression. The complete
numerical gate remains failed, with all original thresholds unchanged.

These are fresh incremental effects against the combined stack, not the total
gain against the older engine. The earlier 10.0% / 5.4% medians used different
code and execution conditions and must not be added or multiplied into this
result. The cleanup survives cross-integration as a useful candidate; its full
production-performance claim remains unadmitted.

## Retention and next comparison

Failure to establish every admission bound is not evidence that the idea has no
value. No cell's median or p95 lower bound establishes a latency regression above
2%; this also does not prove neutrality. The measured target median gains,
unchanged output and small product change justify preserving this candidate for
further integration. The failed tail and RSS gates remain visible.

The next candidate is the separate glyph-hull memo port. Compare the combined
stack plus this clip cleanup against that same stack plus the memo, with fresh
source/binary bindings and independent output checks. That pair measures the
memo's additional value; it cannot be substituted for a cumulative comparison
against an older engine or competitors. The unapplied port and its bindings are
retained beside this record. Its historical 2,064-byte object, including when
disabled, needs current-target layout and memory evaluation before adoption.

## Integrated validation

The complete `python3 tools/check.py --jobs 1` passed on 2026-10-21 in
1,027.45 seconds: policy, fixture/data verification, formatting, Clippy,
workspace/doc tests, documentation, native/WASM builds, host checks, 196 Python
tests and all 1,237 generated-PDF conformance cases. The completed runner receipt
records exit zero and unchanged source SHA-256
`511edc3ebc862398defd7c795f2fffbb2d71094ebf2b21b722aa2fcb1da016b2`.
The [record](stacked-clip-experiment.json) binds the retained log and receipt.
This validates the integrated clip candidate and lifecycle repair; it neither
validates the unapplied memo nor overrides the failed performance admission.
