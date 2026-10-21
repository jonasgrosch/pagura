# Independent RSS A/A evidence auditor — unexecuted

`audit.py` imports only the standard library. It does not import the producer,
invoke a subprocess, launch rendering, time work, resample, or issue admission.
`test_audit.py` contains eight bounded pure/file tests; neither file has been
executed by its author. The coordinator owns testing and every actual audit run.

The auditor requires exactly 14 historical-binary/cell strata, two warmup plus
200 measured pairs per stratum, seed 20261113, 2,828 pairs, 5,656 outputs and
16,970 journal events. All invocation order, A/B binary identity, stage identity,
Darwin RSS bytes and raw artifacts are independently checked. Every RGBA blob
must equal a strict conversion of the separately frozen RGB8 P6 reference with
alpha 255. The original analytic19 descriptor pins remain hard-coded.

The source/build chain is checked against the original rectangle binding and
its final build receipt and Cargo logs. Current file content is rehashed using
the original source recipes; recorded historical commit identities are not
rewritten to the diagnostic's current runner commit. This is cooperative local
provenance, not proof against a compromised coordinator or host.

Run after finalization, before changing either source worktree or runtime.
Use a new audit output outside the producer evidence directory. Preserve every
failed audit attempt; fixing an audit schema issue may recheck the same retained
samples, but never replace, rerender or resample them.

```
ABS_PYTHON -B ABS/audit.py --pins ABS/audit-pins.json \
  --pins-sha256 INDEPENDENT_SHA256 --output NEW_ABS_AUDIT_JSON
```

The coordinator prepares and separately hashes a JSON object with these fields:

```json
{
  "root": "ABS_CANDIDATE_WORKTREE",
  "directory": "ABS_COMPLETED_RSS_DIRECTORY",
  "bindings": {"path": "ABS_ORIGINAL_RECTANGLE_BINDINGS", "sha256": "HEX"},
  "plan": {"path": "ABS_COMPLETED_RSS_DIRECTORY/plan.json", "sha256": "HEX"},
  "report": {"path": "ABS_COMPLETED_RSS_DIRECTORY/report.json", "sha256": "HEX"},
  "journal": {"path": "ABS_COMPLETED_RSS_DIRECTORY/events.jsonl", "sha256": "HEX"},
  "receipt": {"path": "ABS_FINAL_OUTER_RECEIPT", "sha256": "HEX"},
  "guard": {"path": "ABS_OBSERVER_DIRECTORY/guard.json", "sha256": "HEX"},
  "producer": {"path": "ABS_FROZEN/rss_aa.py", "sha256": "HEX"},
  "producer_argv": ["ABS_PYTHON", "-B", "ABS_FROZEN/rss_aa.py", "--root", "ABS_CANDIDATE_WORKTREE", "--bindings", "ABS_ORIGINAL_RECTANGLE_BINDINGS", "--bindings-sha256", "HEX", "--output", "ABS_COMPLETED_RSS_DIRECTORY"],
  "coordination_sources": {"ABS_ROOT/tools/runner.py": "HEX", "ABS_ROOT/benchmarks/results/sources/analytic19-conversion/load_observer.py": "HEX"},
  "observer_snapshots": {"ABS_OBSERVER_DIRECTORY/snapshot-NNNN.txt": "HEX", "ABS_OBSERVER_DIRECTORY/snapshot-MMMM.txt": "HEX"}
}
```

Paths must be absolute. `producer_argv` is the exact externally frozen direct
invocation, identical to the strict guard command and producer `sys.orig_argv`.
The outer receipt must bind **plan, report, journal and guard** after completion.
The copied `runner-start.json` is checked only as a running snapshot, never used
as terminal evidence. Independently pinned preflight/postflight process snapshots
are required; strict observer records must have no known-contention matches.
Observer polling cannot prove the absence of short or unrecognized other work.

The audit is deliberately limited to complete diagnostics. Failure, cancellation,
deadline expiry, journal error, truncation, missing records, changed pins, changed
source/runtime or any pixel discrepancy raises an error instead of producing a
verified result. Failed producer evidence remains retained; there is no partial
success path, allowance or repair. All measured RSS values, sampled maxima,
inclusive quartiles and order differences are recomputed. Timings are validated
as raw descriptive counters only. The diagnostic cannot erase the original
rectangle pilot/confirmation misses or establish an absolute memory bound.
