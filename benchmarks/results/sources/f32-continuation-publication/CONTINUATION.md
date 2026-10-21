# Publish the amended two-cohort confirmation

The original `publish.py` and decision template remain unchanged. This separate
`publish_continuation.py` reads the completed, independently audited continuation
only after the coordinator supplies a fresh pinned decision using
`decision-continuation.template.json`. It launches no render, sample or bootstrap.

```text
PINNED_PYTHON publish_continuation.py --decision ABS_DECISION --decision-sha256 SHA
```

Run from the unchanged candidate checkout under the existing runner and strict
observer. The final publication receipt should bind this script, decision,
generated JSON/Markdown and final observer. The audit child it authenticates is:
`PINNED_PYTHON benchmarks/evaluate_f32_painted_interval_continuation.py confirmation-001 --config ABS_CONFIG --config-sha256 SHA`.

It reuses continuation `validate()` to authenticate the original failed
receipt/guard, explicit user amendment, immutable core, input prerequisites and
new plan/config. The final continuation measurement and audit must have completed
under clean strict observers; this does **not** make the original guard clean.
The original guard recorded another process being present, not its CPU activity
or a measured timing effect. Neither zero CPU nor proven timing contamination is
claimed. All structurally complete original pairs are retained without timing
selection. Any terminal incomplete attempt remains recorded and that pair is
rerun in full; no completed pair is substituted or duplicated in statistics.

Exclusive outputs are
`benchmarks/results/f32-painted-interval-continuation-experiment.{json,md}`.
They retain all 56 cells, unchanged target/protected median and p95 thresholds,
all latency/RSS misses, lower-bound signals above1.02, and min/median/p95/max RSS
from all200 measured samples per arm/cell. JSON keeps original/continuation raw
segment descriptors, every pair's cohort mapping and abandoned-attempt records;
each cell also gets warmup/measured cohort counts. Original pair commands and
blob paths remain unchanged in the pinned combined/raw reports.

`retain-experimentally` requires a nonempty coordinator reason/restriction, all
quality/provenance gates, and at least one original target median upper bound
at or below0.95. `reject` is also supported. Neither decision is automatic, and
neither admits to main/default/release. The pooled numerical pass is separately
named `amended_numeric_gates_pass`; `full_experiment_pass` and
`original_uninterrupted_protocol_pass` remain false. A two-cohort time boundary
cannot be removed by passing the unchanged numerical thresholds. No pilot or
historical samples are pooled; no additional sampling is authorized.

Bounds remain the fixed harness's bootstrap results. The auditor independently
checks raw outputs, schedules, point statistics and bindings; it does not
independently recompute bootstrap bounds. Drafted without execution or imports.
