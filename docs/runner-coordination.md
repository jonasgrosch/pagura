# Cooperative runner coordination

`tools/runner.py` runs one foreground command under the existing Pagura runner
locks and writes a local lifecycle receipt. It coordinates participants that
honor those locks. It does not reserve the entire machine, inspect other jobs,
limit CPU usage, establish an idle window, or attest thermal/power conditions.
It never sets a benchmark's `runner.controlled` flag.

The wrapper uses POSIX file locks, signals and process groups on the current
macOS/Linux runners; it is not a Windows runner implementation.
The Python runtime must expose `waitid(WNOWAIT)`; unsupported runtimes fail before
launch. The wrapper and measurement helper retain their child leader unreaped
while checking its original process group, then disable forwarding before reap.
This prevents a recycled numeric process-group ID from becoming a signal target.
Permission errors and failed membership inspection invalidate execution evidence.
This is a cooperative foreground-command contract, not containment of descendants
that deliberately leave the group or race process snapshots. If cleanup becomes
unverifiable, the coordinator must resolve it by process inspection before another
job; a failed receipt alone is not proof that resources are free.

The coordinator must first establish an available runner or agreed local window
and freeze the source being validated. Reading or editing unrelated work remains
possible, but edits to the measured source invalidate its source binding. The
wrapper does not obtain permission, contact another task, or discover a runner.

## Commands

From the checkout being validated, choose a new receipt path for each run:

```sh
python3 tools/runner.py --owner coordinator --task integrated-check \
  --kind validation --receipt benchmarks/results/local/check-001/runner.json \
  -- python3 tools/check.py --jobs 1
```

`check.py --jobs 1` separately bounds Cargo build concurrency, Rust test harness
threads and conformance workers. Host/browser workers still exist. The wrapper
does not reinterpret or add concurrency flags.

For a benchmark, explicitly name each final report to bind:

```sh
python3 tools/runner.py --owner coordinator --task paired-conversion \
  --kind benchmark --receipt benchmarks/results/local/paired-001/runner.json \
  --artifact benchmarks/results/local/paired-001/conversion.json \
  -- python3 tools/bench.py convert --output benchmarks/results/local/paired-001/conversion.json
```

Benchmark jobs require at least one `--artifact`. The wrapper hashes exactly
those files after the command ends; it does not discover reports or claim every
named artifact was newly produced. Consumers must check the embedded run ID and
other evidence bindings. A missing/unreadable artifact fails the wrapper even
if the child exited successfully. The receipt cannot be its own artifact.

`--dry-run` prints argv, cwd, the two environment overrides and declared output
paths. It runs no subprocesses, acquires no locks and writes no files. `--cwd`
defaults to this wrapper's checkout; relative receipt and artifact paths are
resolved against it. Source identity always describes the checkout containing
the wrapper, recorded separately as `source_root`. Use that checkout's wrapper
when validating its source.

## Shared locks and lifecycle

The installed fleet helper uses `/tmp/pagura-bench.lock` with `flock` when the
CLI exists, or `/tmp/pagura-bench.lockdir` with `mkdir` otherwise. The wrapper
acquires the directory, then the file lock through Python's POSIX `fcntl` API.
Both acquisitions are nonblocking. Contention exits with code 125 before any
command or source-identification subprocess runs. Only its own partial
acquisition is cleaned up. The file remains on disk after release: deleting it
could allow two different locked inodes at the same pathname.

Do not nest this wrapper inside `fleet bench`, or another wrapper invocation:
the outer invocation already holds a non-reentrant lock. A future fleet caller
can invoke it through `fleet <runner> run` instead. Existing fleet `run` and
`check` commands alone do not participate in locking. A wrapper cannot exclude
participants that ignore these conventions.

The lock directory records a run ID, owner, PID and hostname. These identify an
owner for review; an old timestamp or absent PID is not permission to take over.
There is no expiry, timeout takeover, automatic stale-lock deletion, or killing
of unrelated processes. A separately agreed reservation expiry does not release
a lock while work continues; reservation validity belongs to separate evidence.

The command starts in its own process group. SIGINT and SIGTERM received by the
wrapper are forwarded only to that group. The wrapper waits for the direct child
and remaining group members before releasing locks. It does not escalate to
SIGKILL. If the group cannot be confirmed absent, it retains coordination and
continues waiting; further received cancellation signals are forwarded again.
Receipt update failures after launch do not skip waiting for the child.

Only foreground commands are supported. Commands that deliberately daemonize or
leave their process group cannot be tracked by this small wrapper. Its file-lock
descriptor is inherited by the direct child to reduce the orphaning risk if the
wrapper crashes, but arbitrary descendants can close inherited descriptors. An
uncatchable termination may leave an incomplete receipt and stale directory;
the coordinator must establish what still runs and resolve that state manually.
The legacy file-only protocol cannot guarantee exclusion after every such crash.

The standard benchmark measurement helper starts each renderer in a separate
session for timeout ownership. That helper must install cancellation handlers
before launch, forward received cancellation into its renderer group, wait for
cleanup, restore its handlers and abort the benchmark before another sample.
The outer wrapper cannot perform that cleanup itself. The inherited file-lock
descriptor does not solve this: the measurement subprocess closes unrelated
descriptors by default. The Python-only lifecycle test exercises this boundary
through a renderer and its worker, without running a PDF engine.

## Receipt and report binding

The schema-1 JSON receipt is created exclusively; an existing receipt is never
overwritten. Subsequent updates use atomic replacement and check the run ID.
The on-disk state remains running until lock cleanup has a known outcome;
only then is the terminal receipt written. A failed final write leaves incomplete
evidence rather than an earlier completed receipt.
It records:

- Identity: run UUID, owner/task, job kind, exact argv/cwd, source root and runner
  hostname/system/machine/PID, plus the shared lock paths and acquisition time.
- Provenance: HEAD and the benchmark source digest before and after execution.
  Matching boundaries detect some drift; they do not prove no intermediate
  edits occurred. Source freeze and executable identities remain necessary.
- Lifecycle: `running`, then `completed`, `failed`, `canceled`, `spawn-failed`
  or `evidence-failed`; start/end/finalization times, monotonic duration, child
  PID, raw child exit code, wrapper exit code and cancellation signal.
- Outputs: final `artifacts` mapping absolute paths to SHA-256 digests, explicit
  per-artifact errors and other evidence failures.

The wrapper adds only `PAGURA_RUN_ID` and `PAGURA_RUN_RECEIPT` to the inherited
environment. It does not record the full environment or expose credentials.
Reports may store those two pointers. The final receipt hashes the report; the
report must not hash that same final receipt, which would create a circular
binding. A receipt's existence or a report pointer alone is not verified proof.

Gate integration requires an externally pinned final receipt digest, the
matching report run ID/path/hash, `status == "completed"`, both exit codes zero,
and equal before/after/current/measured source digests. `running`, cancellation,
missing bindings, changed source and artifact failures cannot pass. Runner
attestation and all existing accuracy/performance gates remain separate.

After the wrapper exits, review its final receipt and obtain its digest outside
the candidate report. Then pass that pin explicitly:

```sh
python3 tools/bench.py gate benchmarks/results/local/paired-001/conversion.json \
  --runner-receipt benchmarks/results/local/paired-001/runner.json \
  --runner-receipt-sha256 REVIEWED_RECEIPT_SHA256
```

This conversion example remains exploratory and blocked even with intact
bindings. Release artifact admission currently handles the render-e2e track;
matrix aggregation and dedicated-runner attestation remain unimplemented.
Pinning a locally produced receipt establishes consistency, not authenticity
against a malicious runner. Do not treat the report's stored gate result,
computed before wrapper finalization, as a fresh receipt-verified evaluation.

Child failures retain their nonzero exit code; signal exits become `128 + signal`
at the wrapper boundary. Receipt/coordination/evidence errors use code 125.
Cancellation uses `128 + received signal`, even if the child exits zero. A failed
final receipt write cannot produce a valid finalized receipt and returns 125.
No successful receipt alone certifies PDF correctness or performance leadership.
