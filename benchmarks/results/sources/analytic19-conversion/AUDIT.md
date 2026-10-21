# Post-run audit design (standalone draft implemented; not executed)

The inputs were delivered and frozen before measurement. `audit.py` and its
synthetic tests were then authored against the unchanged report schema without
reading campaign measurements. The coordinator owns testing and execution.
The existing helpers below cover authenticated success evidence, but reject
missing outputs by design. Failure receipts therefore need a separate explicit
classification path; treating every verifier exception as a missing sample
would conceal the distinction between engine failure and broken evidence.

## Trusted inputs

- This directory's pinned `manifest.json`, `references.json`, copied `oracles/`,
  original selector hash and README protocol (19 cases, 6 rivals, 2+5 pairs).
- The externally supplied final report path and completed runner receipt path
  and SHA-256. Never obtain the expected receipt hash from the report itself.
- Frozen source identity, policy and normalizer hashes; the actual measured
  executable/runtime identities in the authenticated report, rehashed on disk.
- The report is an input to verification, never the oracle or expected matrix.

Use bounded regular-file reads for all JSON/images/receipts. Require exact types
and unique JSON keys. Reject unexpected, duplicate or escaping paths and labels.
Do not import or execute competitor modules to discover identity or correctness.

## Pass A: provenance and complete attempt ledger

1. Authenticate outer receipt/report with
   `benchkit.runner_evidence.verify_runner_receipt`. If incomplete/cancelled,
   produce an incomplete audit with its recorded reason; never substitute a
   successful per-cell record for completion of the fixed protocol.
2. Match all 114 `(case, competitor)` cells exactly, including unavailable
   competitors. For each available cell require warmup pair IDs `[-2,-1]` and
   measured pair IDs `[0,1,2,3,4]`, with two named results per pair, legal order,
   and no extra/duplicate/missing pair or result. Check page=0, DPI=72 and the
   pinned PDF for every case. An unavailable cell must retain its identity
   reason and empty sample arrays; never count its planned invocations as run.
3. Derive expected invocation settings from the protocol: Exact/full Pagura,
   white output, original page geometry, declared annotations and single-page
   source. Match commands to the current adapter command constructors, including
   the input snapshot and output path. For available identities use
   `benchkit.artifacts.verify_identity` to verify executable/runtime/adapter
   hashes; authenticate its identity digest in every execution context.
4. Count separately: planned invocations, launched attempts with receipt,
   unavailable engines, prelaunch refusals, cancellations, missing ledger
   entries, output-bearing attempts and output-missing attempts. A report may
   retain an engine failure whose output is absent; that is an explicit failed
   attempt, not a successful render or permission to discard it. Any artifact
   store incompleteness also invalidates evidence completeness.
5. For every binding authenticate the immutable receipt hash, context hash,
   run ID and recorded measurement against the authenticated report. Input
   snapshots must match the external PDF hash. The standalone auditor checks
   receipt/context/measurement and raw image fields directly, using a separately
   implemented image parser; it does not depend on the success-only image
   verifier to classify failure receipts. Generic conversion has empty context references; the
   external reference check comes in Pass B, not by inventing a missing receipt
   reference field. For failures, preserve the raw receipt and any retained blob;
   separately report missing/malformed output and verification errors. Never
   turn a verifier exception into a claim of valid retained output.

## Pass B: image correctness independent of engine agreement

1. For each external case authenticate the copied `oracles/<original-basename>`
   against its original `oracle_sha256`, parse the P6 header and exact RGB bytes,
   and verify dimensions against the original descriptor. All 19 frozen cases
   are one page at 72 DPI, with Exact tolerance zero; this was checked when the
   input package was prepared. No rescaling or alignment is permitted.
2. Re-read every existing retained raw blob, including warmups and erroneous
   outputs, and verify raw hash and bounded declared dimensions. Independently
   normalize P6, supported P7 or RGBA8 under the recorded white-background rule.
   Preserve RGBA alpha bytes and report alpha min/max/nonopaque counts only if
   they are actually encoded. P6 has `alpha_status: unavailable`; do not append
   invented 255 bytes and call them measured alpha.
3. Compare normalized RGB with the immutable oracle. Record all channel errors,
   dimension failures, raw/normalized hashes and pass/fail under the predeclared
   bound. Reconcile the independently recomputed result with the report status.
   An unsupported/timeout/failed process stays failed even if its partial image
   happens to match. A mismatch between recorded and audited status is a
   separate harness/evidence finding.
4. Audit the four image-direct Exact descriptors against their source/policy/
   settings/derivation pins. The draft validates each external descriptor's
   complete source set and native Exact invocation once, then authenticates the
   PDF and independently compares every corresponding retained image with the
   descriptor's pinned oracle. It does not regenerate or replace an oracle.
   Competitors are compared with the same frozen RGB oracle; do not claim they
   executed Pagura's Exact mode or that another sampling policy violates PDF.
5. Count independently correct outputs, incorrect outputs, unsupported/failed
   processes, unverified/missing outputs and alpha availability by engine/case.
   Rival-to-Pagura agreement is secondary diagnostic information only.

## Pass C: point estimates without favourable filtering

1. Validate every measured `elapsed_ns` as a positive integer. For the five
   stage-reporting competitors and Pagura validate nonnegative integer stage
   fields and `engine_ns == open_ns + render_ns`; require
   `open_ns + render_ns + emit_ns <= total_ns <= elapsed_ns`. A zero internal
   metric is unavailable for a ratio, never replaced by a small positive value.
   Preserve Poppler stage-unavailable with its process/RSS measurements.
2. Recompute each side's median and p95 from all five measured samples, using
   the existing linear quantile definition `p*(n-1)`. Compute ratios of those
   point estimates for process, open+render and render, all three reported
   together; also show open and emit for attribution. Do not replace paired
   observations, take minima or select the favourable metric/case/engine.
3. Only label a ratio as accuracy-qualified when the entire fixed cell has
   authenticated, independently correct measured outputs and valid timings.
   Require successful authenticated warmups too for the declared protocol.
   Otherwise report the failure counts and mark the cell comparison invalid;
   any complete raw timing distributions may be shown only as explicitly
   unqualified diagnostics, never as a ratio computed from a success subset.
4. Recompute maximum measured process RSS per side and ratio only when all
   expected RSS values are positive integers. Preserve missing RSS. This is
   runtime/process memory, not renderer-only heap or stage memory.
5. Do not claim an independently replayed timing record: successful raw logs
   were not retained. This pass authenticates stored stage values and checks
   their arithmetic. It does not reconstruct discarded logs, replay clocks or
   certify dedicated hardware. Do not recompute or claim calibrated confidence
   intervals from five observations; retain any harness bootstrap fields with
   their existing exploratory caveat.

## Minimal audit regression inputs before use

Author small synthetic receipts/blobs (no renderer execution) covering:

- Correct P6 output, correct premultiplied RGBA over white and straight RGBA;
  encoded alpha preserved versus P6 alpha explicitly unavailable.
- Corrupted raw hash, corrupted independent oracle, swapped case/page/DPI,
  duplicate/missing warmup or sample, wrong engine prefix/identity hash.
- A failed child with a valid partial image; missing-output failure receipt;
  unavailable competitor; artifact capture failure and incomplete outer receipt.
- Incorrect output retained among otherwise correct pairs; prove no subset
  median/ratio is substituted. Malformed/zero stages and unavailable Poppler
  stages must not fabricate internal timing data.

The audit's final status should distinguish completed-and-audited evidence from
accuracy success: a complete honest run can reveal incorrect rendering. Neither
status grants release admission or a general fastest/most-accurate claim.
