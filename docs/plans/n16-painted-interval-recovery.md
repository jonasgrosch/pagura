# Painted-interval external-contention recovery

This is a separately named experiment, `n16-painted-interval-recovery`,
declared before collecting its samples. It does not extend the launch budget
of `n16-painted-interval` or revise the renderer. The original confirmation's
one-launch allowance is exhausted; its admission result is unavailable.

## Reason and excluded evidence

The original `confirmation-001` was cancelled at 2026-10-21T15:27:30Z when
the strict observer detected a foreign `cargo test -q -p pagura-document`
and its compiler children. Its final receipt reports exit 143, unchanged
source, and no artifact errors. The report, receipt, observer snapshots,
launch configuration and launch intent remain at
`benchmarks/results/local/n16-painted-interval/`.

Three protected cells completed before cancellation; neither target completed.
The coordinator has not inspected their numeric samples. Every sample from
that run is excluded from inference and retention, including samples collected
before the detected contention. This recovery was selected because of the
observed external interruption, independently of partial timing outcomes.

The user instructed the coordinator to wait for other jobs and continue.
The original plan explicitly permits separately named, predeclared revisions.
This record authorizes one such recovery within that standing instruction;
there is no automatic retry loop or permission for outcome-driven resampling.

## Frozen recovery contract

- Reuse exactly the original 18 cases, 52 cells, two target roles, 50 protected
  roles, independent oracles, products, binaries, build records and metrics.
- Run one complete fresh confirmation with 200 measured pairs and five warmup
  pairs per cell: 21,320 independently checked outputs. Use order seed
  `20261108`; retain bootstrap seed `42017`, 5,000 resamples and one-sided 95%
  marginal bounds.
- Keep every latency, memory, accuracy, resource, layout, provenance,
  portability and partial-retention rule from the original plan unchanged.
  Preserve and report all protected and RSS misses. A partial win is an
  experimental retention decision, not full admission or release evidence.
- Do not pool original confirmation, pilot, WASI, historical or diagnostic
  samples. The completed original pilot and WASI audit remain prerequisites
  through their original plan/binding hashes and unchanged product identities.
- Freeze new plan, bindings, configuration and driver/auditor source hashes
  before launch. Record one exclusive intent in the new experiment directory.
  Preserve all original frozen files byte-for-byte.
- The coordinator owns the sole compute job. Wait for three quiet scans,
  acquire the existing cooperative runner lease, and retain the strict
  five-second known-process observer and original limits throughout the run.
  Do not stop or alter foreign jobs. A guard cancellation invalidates this
  complete recovery too; no automatic retry or added pairs follow.
- Audit raw outputs, fixture identities, paired order, measurements and the
  receipt/guard/driver/intent chain before making a retention decision.

The existing complete quality record, actual native/WASI layout observations,
resource/error transcripts, precision checks and 24-case revised structural
reach evidence apply to the unchanged product. This recovery only replaces
unavailable speed and sampled RSS evidence. It establishes no competitor,
worldwide, cold-I/O, latest-main or overall-stack superiority claim.

## Original immutable anchors

| Artifact | SHA-256 |
| --- | --- |
| original plan | `84d8932bcc23089b71d44ff5e513f9a549e4cff77aa57b7825544149680b55d8` |
| original bindings | `1d1068bcfe81843728d29c1b5267d3207bfb3a3d6f03d3bdf1196f35641e9e6a` |
| interrupted receipt | `7123383335df77dbd0902961976a65ce9228ccddaa32c6142e579c489f519528` |
| interrupted guard | `8dd70cc5b000baa05251badc2c1f70182f4776500f7752f83db1eef36f1a7dc6` |

The preparation helper verifies and records these anchors before writing new
files; an inconsistent anchor blocks the recovery launch.
