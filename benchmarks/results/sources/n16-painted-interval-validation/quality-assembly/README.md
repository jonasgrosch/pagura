# N16 pilot quality assembly recipe

Source-only coordinator recipe; nothing here has been executed. This is not a
new collector, a validation result, or permission to replace any frozen gate.
Fill `quality.pending.json` only from completed evidence. Its pending values
deliberately cannot satisfy `launch_n16_painted_interval.py`.

## Inputs and provenance

For each evidence job retain an externally recorded `{path, sha256}` for the
FINAL outer runner receipt, strict observer report, primary report, raw logs,
input/output artifacts, executed helper sources, runtime/toolchain and build
records. Use absolute resolved paths. A report's self-digest or the archived
`runner-start.json` is not the final receipt. Preserve failed/cancelled attempts
in `attempts`; none can provide a passing gate.

Before using a successful job:

1. Read only bounded regular files; check each pinned file's actual bytes,
   SHA-256 and recorded size. The final receipt must bind the report and guard;
   their pinned artifact inventories must bind every underlying raw artifact.
2. Require receipt `status=completed`, exact integer `exit_code=0` and
   `wrapper_exit_code=0`, `source_unchanged=true`, equal before/after snapshots,
   no errors/artifact_errors, and no cancellation signal. Require the expected
   source root, run ID and actual command. Do not equate Python `true` with 1.
3. Require strict guard `status=completed`, `observation_only=false`, exact
   integer exit/child_exit 0, no cancellation, nonempty events with preflight
   and postflight, and every event `matches=[]`. Check retained snapshot pins.
   Follow exact receipt argv -> guard command -> executed helper argv; any
   launcher wrapper needs its pinned execution record. Pin both requested and
   actual Python executables when the framework launcher changes argv[0].
4. Verify every source/helper/runtime pin and the source-to-binary build link.
   Separate production, layout/Q4 instrumentation and reach instrumentation.
   Do not compare distinct digest algorithms as if they were interchangeable:
   runner snapshots are `{commit,sha256}` from `source_digest`; benchmark
   identities are `{root,commit,source_sha256}` from `source_identity`.

The coordinator must freeze mandatory inputs before assembling a passing
record. Missing, malformed, changed, failed, cancelled or unavailable evidence
leaves the corresponding gate pending/failed and blocks pilot. No inference
from a previous candidate, a launch, or a green subprocess alone.

## A. Authoritative full check on restored production

Either use the final successful `main-reconciliation/check.py` report, raw log,
receipt and guard for the exact candidate. Require `status=command-passed`,
`source_unchanged=true`, equal report before/after strings equal to the runner's
source SHA, correct driver hash, and actual `tools/check.py --jobs 1` command
without `--dry-run` or a shortened command list. Check the pinned `check.py`
source's complete sequential command list against the log (currently 14
`Checking:` stages), no omitted stage, and its final marker:

`All checks passed. This is not a complete PDF conformance or performance certification.`

The report process must have exit code 0 and not be timed out. The existing
`run_process` success result can carry provisional `status=failed` with exit 0;
do not reject that field alone or accept a nonzero exit because a summary says
passed. Reject a truncated log. Retain the failed earlier check attempts.

Alternatively, use the archived staged-check coordinator's completed aggregate
and all of its frozen inputs and per-stage evidence. Require all 14 exact
`tools/check.py.planned_commands(jobs=1)` command/environment pairs, completed
serially in that order on the identical frozen source/HEAD, helpers, runtimes,
fixtures and configuration. Every stage needs its own final successful runner
receipt, clean strict guard, complete raw log and before/after source checks.
Recheck the aggregate's evidence rather than trusting its status alone. Retain
every attempt. External contention/cancellation may be retried; an unresolved
semantic failure, timeout, source drift or evidence failure blocks that freeze.
Missing/duplicate stages or a partial earlier whole-check run cannot satisfy
this route. Describe the result as a complete staged check, with no claim that
one uninterrupted `check.py` invocation passed. The authoritative scope and all
accuracy/performance gates are unchanged.

PI-Q5 additionally requires the current precision/native-WASI host evidence
specified by the plan. `tools/host_test.mjs` includes classic/render/reading
checks but does not replace the two direct-root rational host fixtures.
`opaque_host_probe.mjs` must have both cases passed, native/WASI response bytes
identical, empty diagnostics, alpha 255 and strict rational RGB intervals
218..219 / 5..6; bind current host binaries and their build receipts. This is
Exact host coverage only. Native/WASI renderer tests cover Competitive; do not
invent a Competitive host mode. `opaque_precision_probe.py` exits successfully
even for a recorded pixel mismatch: if used, require all 8 rows, all individual
`within_contract=true`, `all_within_contract=true`, and full/tile equality.

## B. Six-arm layout and Q4 diagnostics

Use the final `run_v3.py` report with `status=verified-tests-and-diagnostics`,
`layout_gate_passed=true`, and exactly baseline/candidate x
native/wasi-simd/wasi-scalar observations. Recheck its raw records, not only
these summary flags:

- Pinned preparation, patches, executed archived `code/run.py` and WASI
  wrapper; expected installed identities equal before/after; fixture inventories
  nonempty, equal across arms, and unchanged. Pin all build/runtime overrides.
- Six builds and six test invocations, each returned with exit 0. Build logs
  have exactly one successful `build-finished` and one `pagura_render` lib-test
  executable. Each lies in its own arm/platform target directory and matches
  the archived binary bytes/hash. Extra runtime-query commands are expected;
  do not mistake the entire `commands` length for 12.
- Reparse all six raw test logs using the pinned producer's grammar. Exactly
  one successful summary, zero failed/ignored/measured/filtered tests, positive
  count equal to the unique actual test names, required tests present. Candidate
  inventories include both public painted-interval/tile/reuse tests. Logs must
  be complete UTF-8 without replacement characters or the 16 MiB cap reached.
- Each log has exactly eight distinct layout rows: Buffer, Option<Buffer>,
  Surface, MaskSink, Context, Vec<[u16;4]>, Range<usize>, [u16;4]. Require actual
  native target/pointer width and actual WASI wasm32/32-bit SIMD true or false.
  Pinned Rust prints an empty WASI OS string; verify compiler target cfg and
  executed module rather than replacing this observation with a host guess.
- The WASI metadata binds the actual archived module, Node executable, empty
  runtime exec argv, `--nocapture --test-threads=1`, and the matching arm-root
  preopen. Preserve its writable-preopen limitation and mutation checks.
- Each raw log has 47 Q4 rows with category counts: page 4, output 2, source 20,
  function 3, command 1, singular 2, none-stroke 2, depth 2, inverse-overflow 2,
  f32-function 1, region 6, translucent-source 2. Compare the complete ordered
  baseline/candidate strings on each platform, including error/context/counters.
- Recompute every observed size/alignment delta. All eight recorded types must
  satisfy the producer's non-growth check. For the launcher's eight layout
  rows, take the four enclosing types on native and WASI. First require both
  WASI configurations to agree per arm/type; otherwise report unavailable and
  review rather than silently selecting one. Rename raw `align` to `alignment`.
  Require positive exact integers; do not infer Words/enum layout from two
  added usize fields. Retain the additional raw Context/payload observations.

Source review must establish that temporary cfg(test) probes do not change the
observed types or layout-affecting cfgs versus production. The tests/logs must
cover the PI-Q1/Q2/Q3 named assertions below; a count alone proves no such scope.

## C. Fixed native route/reach observation

Require the coordinator reach report to have `schema=1`,
`validation=n16-interval-route-reach`, `status=verified-route-reach`,
`expected_outputs=completed_outputs=18`, both aggregate check flags true and
exactly the fixed 9 requests x Exact/Competitive. Each is full/72 DPI:
opaque64, opaque-empty, opaque-distant, opaque-fill, opaque-tiny, sparse64,
default-knockout64, image-only on white; opaque-empty on [17,85,170,128].
Both sparse64 and image-only are F32 controls: image-only's quarter opacity
precludes the opaque-image shortcut. No bypass is permitted in this fixed
18-request scope; an observed bypass remains a route miss.

Bind the actual current harness PDF/oracle bytes, diagnostic patch/source,
dedicated target directory build, binary, command, complete raw stderr/output,
and final receipt/guard. Reparse every marker and raw RGBA using the pinned
reach README's rules: one actual plan, correct policy/background, exact chunk
partition, real surface/writer/output records, no bypass, validated writer union,
and actual converter row intersections. Require exact independent pixels and
opaque alpha; the nonopaque empty control is [9,43,85,128]. No oracle comes from
the renderer. Any additional zero halo or route/count discrepancy is a retained
reach miss, not permission to adjust the fixed expected counts after observing
them. Native full-route evidence does not establish all 52 cells or WASI reach.

## D. Restoration and gate mapping

After EACH temporary overlay, pin an explicit restoration record: the exact
patch and every touched file's pre/install/post hashes, pre/post production
benchmark identity and runner snapshot, unchanged fixture/helper closure,
and source-diff review. Require exact original byte equality and identity
equality after removal; don't merely assert `git diff` is empty. Never carry
probe identity into timed build records. If production changed since a check,
do not silently reuse it; record an explicit reviewed correspondence or rerun
the required check under coordinator control before marking its gate passed.

| Gate | Minimum support to record |
| --- | --- |
| PI-Q1 | Candidate test inventories and assertions for interval initialization, both N16 writers, clipped/failing rows, dense arbitrary-destination output and exhaustive 65,536-word quantization; unchanged writer inventory review. |
| PI-Q2 | Cross-row/stride/hole test, independent irregular/offset public tile test and actual Competitive output-storage reuse test; dense F32 and legacy U8 controls. |
| PI-Q3 | Premultiplied background/full/tile tests and direct-root rational tests in both modes, native and both WASI configurations; nonopaque F64 guard. |
| PI-Q4 | All same-platform 47-row baseline/candidate error/resource transcripts plus required source/work/function/depth tests. |
| PI-Q5 | Exact current production authoritative full check plus required precision/native-WASI host evidence and both-mode renderer tests. |
| PI-M1 | Actual six-arm enclosing size/alignment observations, cfg-definition review and byte/heap/resource invariant review; no RSS or peak-stack claim. |
| PI-REACH | All 18 fixed native requests and their raw route/count/pixel checks, followed by exact restoration. |

## E. Final assembly and pilot handoff

Use a small final read-only coordinator summary job under the existing runner
on restored production. Fill `quality.pending.json` into a NEW quality artifact;
set each gate to passed only after its evidence/check list is satisfied. Record
the exact candidate runner snapshot in `source`, eight actual layout rows and
all evidence pins/review notes. The summary job executed aggregation, not the
underlying tests. Preserve that limitation. Do not write its own final receipt
hash inside itself; the outer receipt is finalized after the child exits.

Then independently pin that completed summary receipt and quality artifact in
`config.prerequisites.quality.{receipt,artifact}`. Its receipt must bind the
artifact, use the candidate root, and have `source_before=source_after` equal
to `bindings.runner_sources.candidate`. Check this after any permitted evidence
publication/commit; a commit change also changes the runner snapshot.

Freeze all final source/build/fixture/helper/plan/binding/config pins. Verify the
launcher's `--phase pilot --dry-run` under the coordinator before the sole new
launch; it writes/launches nothing. The 104-output uninstrumented all-cell
preflight remains separately required. Pilot stays 52 cells, 5 warmup + 5
measured pairs, seed 20261105, 1,040 outputs. No threshold, reference, count,
retention policy, automatic retry or failed-attempt deletion changes here.
