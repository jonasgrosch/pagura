# Conditional clip viewport experiment

Status: **promising partial result, not yet admitted; preserved as a patch outside
the renderer**. Following the user's instruction to test immediately, the fixed
200-pair-per-cell run completed while background compute was observed. Its
8,200 outputs all matched the independent reference and same-mode baseline bytes.
The complete numeric speed/memory gates failed; the environment gate also fails.
This withholds promotion under the frozen experiment rules; it does not retire
the idea or establish a causal regression. The partial win should be evaluated
incrementally on the newer integration stack, as described in the
[stacking review](performance-stacking-review.md).

| Clip-churn full render | Baseline median | Candidate median | Median latency change | Baseline peak RSS | Candidate peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Exact | 5.035 ms | 4.532 ms | -10.0% | 13.578 MiB | 13.500 MiB |
| Competitive | 8.700 ms | 8.231 ms | -5.4% | 10.922 MiB | 10.984 MiB |

Only one of two targets passed its numeric latency bounds; three of eighteen
protected cells passed both latency bounds. Twelve of twenty cells passed the
peak-RSS gate. Competitive's target median upper bound was 0.9542 (required
0.95), and its p95 upper bound was 1.1666 (required 1.00). Exact's corresponding
bounds were 0.9082 and 0.9854. These are marginal bootstrap bounds, not a joint
confidence claim. Contention prevents causal attribution of apparent regressions
or generalizing these observed gains to controlled performance.

The earlier bounded quiet-window wait launched no benchmark. The subsequent
[run-now protocol](performance-run-now-protocol.json) recorded background activity
without aborting. The pilot (400 outputs) and confirmation (8,200 outputs) used
separate fixed samples and were never pooled. No threshold or pixel tolerance
changed. See the [complete cell results](clip-viewport-experiment.json).

The [candidate patch](clip-viewport-candidate.patch) moves viewport path construction
inside the soft-mask-source branch of `Context::clip`. Ordinary clips still use
the page crop; clip ordering, geometry, error paths and cache publication are
unchanged. The baseline renderer had first been restored exactly after removal
of the glyph memo, so this was a separate experiment with one product change.

The release build and existing `independent_content_pixels` test passed. A separate
preflight verified 40 renders across all 20 declared cells: exact independently
derived RGB8, alpha 255, and identical same-mode baseline/candidate RGBA bytes.
These checks cover both modes, full/cold/tile/tiled operations, clip churn,
shared clips, soft-mask sources, dense pages, opaque pages and tiny pages. They
do not replace the full candidate check or a representative corpus comparison.

Static disassembly of the two frozen ARM64 release binaries supports the proposed
mechanism: the ordinary candidate branch skips the path-vector growth call, and
the function stack reservation falls from 1,456 to 1,376 bytes. This is neither
an allocation-count measurement nor evidence of end-to-end speed or peak RSS.

The [frozen plan](clip-viewport-plan.md) requires both targets to meet a 0.95 median
upper bound and 1.00 p95 upper bound, all 18 protected cells to meet 1.02 upper
bounds, and every cell to have peak-RSS ratio at most 1.00. Pixel, identity,
artifact, environment and full-validation gates also apply. None of those gates
was relaxed. The measured confirmation remains exploratory because the execution
gate fails; no timing from the earlier correctness preflight was used.

The [bindings](clip-viewport-bindings.json) identify both source trees and binaries,
the identical public-API example, the fixed plan and all benchmark modules. The
[machine-readable record](clip-viewport-experiment.json) binds the build,
correctness, timing/RSS, scheduling and code-generation artifacts. The candidate binary is
preserved locally at `local/clip-viewport/candidate-viewport`; the patch can be
applied to the restored renderer for a future controlled evaluation.

The shared `paired_render.py` loop preserves the earlier scene fixture definitions,
pairing, retention and output checks. Both entry points record and recheck their
Python entry/helper/fixture hashes. The driver must run from its declared
candidate checkout so its imported tooling is covered by the source identity.
Static fixture-function comparison, syntax checks and four CLI rejection checks
passed. A delayed draft-copy overwrite was restored to the exact frozen bytes
before any benchmark launch, with a separate local record. One trailing blank
line was removed from the shared runner after measurement; its exact measured
bytes remain retained, with [both hashes and AST equivalence recorded](clip-viewport-harness-formatting.json).

Final restored-source validation passed in 985 seconds: the full single-job check,
182 Python tests, 1,237 conformance cases, and native/WASM host parity. The runner
recorded unchanged source identity. This validates the restored renderer and
retained tooling; it does not retrospectively validate or admit the preserved candidate.
