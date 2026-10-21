# Independent start-to-current auditor

Unexecuted source-only draft. It never imports the collector or fixture recipes,
launches a renderer, replaces samples, or grants release admission. The coordinator
owns all execution and final external pins. The timed matrix uses default repair
open plus full compile, not strict open, and does not expose diagnostics. Separate
strict accuracy probes remain a prerequisite for confirmation.

Run after the collector, guard and outer receipt have all finalized:

```text
ABS_PYTHON -B /ABS/audit.py --pins /ABS/pins.json --pins-sha256 HEX --output /ABS/audit.json
```

`pins.example.json` is a concrete schema template, not runnable fabricated pins.
Keep the new audit output outside the measured evidence directory. All pin paths
are absolute. The final receipt must bind report.json, plan.json, events.jsonl and
guard.json. Pin every snapshot named by guard.events separately. Pin the reviewed
guard source and existing tools/benchkit/statistics.py. The binding must additionally
contain `candidate_runner_source_sha256` (the digest from the runner snapshot) and
its completed build evidence must carry the entire final receipt artifact map,
including the guard. Both exact source roots and retained binaries must remain
available and unchanged for this immediate post-run audit.

Confirmation uses the same schema with `phase: "confirmation"` and one extra pin:
`"prerequisites": {"path": "/ABS/preflight-pins.json", "sha256": "HEX"}`.
Its bytes must equal the collector plan's embedded prerequisites. Those pins bind
the completed preflight audit and the independent strict accuracy-probe audit.
The auditor trusts their separately frozen source/coordination review; it does not
reimplement the separate accuracy adapter.

The launch-chain check expects the frozen ordered CLI from collect.py: root,
bindings, bindings-sha256, phase, output, then the two preflight-pins arguments for
confirmation. The outer runner command is Python `-B` guard.py `--evidence-dir`
ABS_DIR `--` that exact collector command. Both the requested Python launcher and
actual framework app must be in the pre-run plan closure; the only permitted
argv[0] replacement is between those two recorded files. No arbitrary interpreter
or post-run replacement is accepted.

The independent 84-cell matrix, seed 20261114 and schedule require all 168 preflight
outputs, or all 34,440 confirmation attempts (5 warmup plus 200 measured pairs per
cell). Every six-event pair is matched to the final report, every retained raw/log
descriptor is hashed, and only then can an independent pixel result be reused for
identical raw SHA-256 + frozen oracle SHA-256 + operation. P6 is interpreted as RGB;
the actual RGBA8 container supplies alpha, which must be exactly 255. No white
normalization hides alpha errors. Tile references crop the literal upper-left
256-pixel rectangle; tiled references remain the complete page.

Every baseline error remains visible. A successful candidate preflight returns
`status: "verified-preflight"`, `candidate_preflight_correct: true`, and the exact
report/binding hashes required by the collector. Candidate pixel/process failure
retains the evidence with exit 1 and `verified-evidence-candidate-failed`; it never
permits confirmation. Missing/malformed provenance or attempts return
`invalid-evidence`, no qualified cells, and exit 1. Interrupted/cancelled/incomplete
runs cannot pass this completed-run audit.

Confirmation reports per-cell correction-cost or equivalent-output qualification,
all warmup and measured quality counts, and all 200 measured pairs without selecting
favorable outputs. The selected latency is total_ns for cold and render_ns for
full/tile/tiled. Both selected latency and RSS get paired median/p95 ratio bounds
using the existing pinned ratio_bounds, 5000 resamples, separate one-sided 0.95
confidence, seed 20261114 restarted per cell. Process/open/compile/render/total
point summaries are all retained separately; maximum RSS has no interval. A missing
metric suppresses that entire paired statistic rather than dropping a pair.

There is no overall speed ratio, no admission decision, no claim of simultaneous
84-cell confidence or adequate tail precision, and no inference that wrong old
pixels establish an equivalent-output speed win. A periodic known-process guard
is cooperative contention evidence, not proof of total machine exclusivity.

`test_audit.py` contains bounded component tests only. None has been run by the
author. The coordinator should run them and the actual completed preflight audit
before relying on confirmation admission.
