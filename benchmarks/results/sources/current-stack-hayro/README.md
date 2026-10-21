# Current combined stack versus Hayro

Freeze product commit `402e3afe`; do not rebase, optimize or change renderer,
adapter, policy, reference tolerances or sample counts during this comparison.
Run the 19 prior analytic cases and all 20 output-interval validation cases.
The first panel preserves the previous comparison; the second directly covers
the workloads that motivated the latest retained changes. Report them separately.

For each of the 39 cases run Pagura Exact and Competitive against the same
standalone Hayro 0.8.0 public-API adapter. Use 100 measured pairs and five warmup
pairs per cell, fixed seed 20261022: 78 cells, 8,190 complete pairs and 16,380
output attempts. Randomize cells and the two engine executions within each pair.
Every execution opens and renders page zero at 72 DPI in a fresh process.
Warmups warm operating-system/filesystem state, not an in-process document cache.

Primary engine time is open plus render, including page loading, compilation,
lazy resources, decoding and output allocation. Output conversion/writing is
the adapter's separate emit stage. Also retain render, open, emit and complete
process times; do not substitute whichever metric looks more favorable.
Per-child `/usr/bin/time -l` reports maximum process RSS, including adapter,
runtime and output emission. Keep all 100 RSS samples, median, p95 and maximum.

Both engines receive identical immutable PDF bytes and are evaluated against
the same independent RGB-over-white references with zero channel tolerance.
The four image-model profiles retain their original source/settings checks.
Competitive is checked against the same common image policy; this does not
claim that its own drawing-only reference contract or every PDF implementation
must use the same resampler. Pagura P6 does not expose alpha; encoded Hayro alpha
does not establish cross-engine alpha superiority.

Preserve every failed, incorrect and unavailable attempt. A timing comparison
is accuracy-qualified only if all measured and warmup outputs from both engines
match. Publish unqualified descriptive measurements with that label instead of
silently removing incorrect cells or substituting a successful subset.

Build the two release binaries sequentially, offline, under the shared runner
and process observer. Hayro uses a private copy of the checked-in adapter and
an isolated target; its dependencies remain outside the product workspace.
Preserve all historical cached binaries and evidence. Record build receipts,
commands, source hashes, executable versions and binary hashes before freezing.

Launch the fixed campaign once under the cooperative lock and strict observer.
A process-presence stop retains all completed pairs and partial attempts; it
does not justify erasing data or asserting measured CPU interference. Any
continuation would require an explicit recorded amendment. A bounded child
timeout preserves the failure and the campaign continues the declared matrix.

The producer writes a flushed journal and immutable content-addressed raw
outputs, including successful diagnostic logs. The independent post-run auditor
checks the exact schedule, commands, receipts, references, pixels, stage logs and
RSS, then recomputes point estimates without launching a renderer. Bootstrap
analysis, if reported, must be labeled with its precise calculation provenance.

The unchanged targets are median and p95 time ratios at most 0.80 and maximum
RSS ratio at most 1.00, with independent correctness. This selected desktop
experiment provides current comparative evidence; it cannot establish the full
release/platform/workload matrix or a worldwide ranking.
