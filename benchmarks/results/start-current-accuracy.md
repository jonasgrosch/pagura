# Session-start to current: accuracy qualification

2026-10-21. The starting product is `359e19dd`, represented by control
`676a872e` with identical production/build sources and the deliberate common
profile-example overlay. The current product is `a9814ee4`, which merges main
`d81d0c82` and retains the experimental renderer stack. This report establishes
bounded native accuracy results. The combined speed and memory confirmation is
now independently audited in [the cumulative report](start-current.md).

## Fixed benchmark preflight

All 168 outputs from the fixed 84-cell matrix were retained and independently
audited. Both versions match the independent RGB, dimensions and opaque alpha
references in 83 cells. In the remaining Competitive translucent-text cell,
the starting version emits 63 instead of 64; the current version is correct.
Its later timing can describe the cost of correcting output, but cannot qualify
as an equivalent-output speed improvement. No cell was removed.

The timing driver uses default repair opening and full compilation through the
public retained-list API. It does not emit repair diagnostics. These preflight
results do not establish strict-open or diagnostic parity.

## Separate strict accuracy probes

The independently audited strict adapter collected 108 outputs: 27 requests,
both modes and both versions. All 54 current outputs pass. The starting product
fails 23 of its 54 outputs. These are deliberately selected regression probes,
including repeated full/tile cases, not 54 representative documents or an
estimate of overall accuracy.

| Correction | Starting behavior | Current behavior |
| --- | --- | --- |
| Two partial gray marks, Exact | 217, outside the independent 218–219 interval | 219, full and tile |
| 1,000 small black contributions, both modes | 127, outside the independent 5–6 interval | 5, full and tile |
| Two half-opacity red marks, Competitive | Incorrect eight-bit accumulation | Independent `[255,64,64,255]` result |
| Eight full-coverage marks at opacity 1/1024, Competitive | Incorrect low-opacity accumulation | Independent `[253,253,253,255]` result |
| Nonopaque colored backgrounds | Failures depend on mode and scene; original Exact blank/opaque backgrounds already pass | All fixed blank, half-coverage and half-opacity controls pass |
| Colored page crossing x=256, Competitive | Incorrect composition in full and offset tiles | Independent color and full/tile crop identity pass |

The root precision changes improve the original Exact product, not merely an
intermediate experiment. F32 preservation closes Competitive errors where Exact
already passed. Background history is mixed: older general/Competitive narrow
routes had defects, and expanding narrow composition also exposed an Exact
migration regression that the guard repairs. Do not count every background fix
as an improvement over initial Exact.

The coverage probes use the declared per-mark coverage-byte model. Passing them
does not establish continuous-scene filtering or preservation of sub-byte
geometry. Both the known limits and the independent rational bounds remain
unchanged. The strict adapter uses public APIs and literal small PDFs; it does
not claim identical dispatch to older private display-list unit tests.

## Evidence and timing status

- [Frozen profile bindings](start-current-bindings.json).
- [Completed preflight audit](start-current-preflight.json),
  covering all 168 raw outputs and the terminal runner/observer chain.
- [Completed strict accuracy audit](start-current-accuracy.json),
  covering all 108 raw outputs, independent formulas, invocation diagnostics,
  baseline failures and full/tile identity.
- [Interrupted confirmation record](local/start-current/confirmation-001-stop.json).
  The observer stopped the trial for external Cargo activity during warmups;
  no measured pairs started and no ratios were computed. Its samples are never
  resumed or pooled into the fresh confirmation.
- [Second interrupted confirmation](local/start-current/confirmation-002-stop.json).
  A new local Cargo check triggered cancellation after 13,420 measured pairs
  completed. No timing statistics were inspected. All samples remain excluded;
  confirmation 003 completed the unchanged protocol after quiet and is the sole
  qualified speed/memory result, with all 34,440 outputs independently audited.

The collector and independent auditor component suites passed 3 and 10 tests;
the separate accuracy auditor passed 6. The product remains unchanged from the
previous fully validated main reconciliation. Native/WASI validation there is
separate evidence; these new start/current probes execute native only.

The next performance candidate is [prepared separately](sources/n16-output-interval-draft/README.md):
conservatively track the written interval of an N16 chunk and omit output conversion outside it.
It is applied only in the separate `codex/renderer-n16-interval` worktree and is
unvalidated. Neither arm of this completed comparison contains that candidate.
