# One bounded, independently oracled conversion pilot

Status: frozen diagnostic pilot, not yet executed. The 14 auditor tests and both
cancellation probes passed; see `preflight.json`. Product baseline is the
validated resource integration `c70b4678`. No adapter or measurement source changes.
This uses the unchanged 19-case selector from `/private/tmp/bench-case-selector`.
`manifest.json` maps those cases into the existing conversion schema;
`references.json` retains their complete original generated case descriptors.
`oracles/` contains byte-for-byte copies of their existing independent PPMs.
All 19 are single-page, 72-DPI cases with declared Exact tolerance zero.

## Scope and fixed pilot

Run the current Exact/default CLI against PDFium, MuPDF, PDF.js, Hayro, Quartz
and Poppler. Freeze 2 warmup pairs + 5 measured pairs per case/competitor, seed
20261021. That is 114 cells and, if all engines are available, 1,596 child
invocations/outputs (1,140 measured + 456 warmup). Unavailable or failed cells
remain visible; do not replace cases, rerun favourable samples or select metrics
after viewing results. Five pairs are a diagnostic pilot, not calibrated tail or
release evidence. The current policy imposes a 30-second child timeout. Use the
coordinator's existing outer cancellation/deadline guard for a 20-minute total
window, including any build; expiry means incomplete evidence and no automatic
retry. Shared-runner coordination is not dedicated-runner certification.

One campaign reports process wall/RSS and internal stages together; a separate
legacy render-e2e campaign would duplicate the outputs and startup costs.

## Command (coordinator only; not run)

Run from `/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura`, with
the normal shared-runner/observer lifecycle and a new receipt path. The wrapper
below uses the cooperative lock, three quiet scans, a strict known-process
contention guard and the existing owned-process cancellation implementation.
`campaign.py` requests cancellation after 1,200 seconds, then waits for owned
work to drain. This is not a hard wall-clock completion guarantee. It forwards
SIGTERM so the conversion process can stop its separately grouped renderer;
there is no outer SIGKILL timer that could kill the cleanup owner first.
The conversion command performs an offline release build before measuring, with
Cargo jobs set to one. The command and all transitive measurement helpers are
frozen in `inputs.json`; engine identities are captured by the existing harness.
Stop on observed competing compute and retain an interrupted trial. No automatic
retry, added pairs or changed cases are authorized by this plan.

```sh
python3 tools/runner.py \
  --owner renderer-stacked-gains --task analytic19-conversion-001 --kind benchmark \
  --receipt benchmarks/results/local/analytic19-conversion/campaign-001-receipt.json \
  --artifact benchmarks/results/local/analytic19-conversion/campaign-001.json \
  --artifact benchmarks/results/local/analytic19-conversion/conversion-001.json \
  --artifact benchmarks/results/sources/analytic19-conversion/inputs.json \
  --artifact benchmarks/results/local/analytic19-conversion/observer-001/guard.json \
  -- python3 benchmarks/results/sources/analytic19-conversion/idle_then.py \
  benchmarks/results/local/analytic19-conversion/idle-001.json \
  python3 benchmarks/results/sources/analytic19-conversion/load_observer.py \
  --evidence-dir benchmarks/results/local/analytic19-conversion/observer-001 \
  -- python3 benchmarks/results/sources/analytic19-conversion/campaign.py
```

The existing `convert` command does not need the new legacy `--cases` option.
Do not add `--accuracy competitive` to this command: this campaign and its
current artifact invocation contract are Exact/default only.

The conversion schema is `{"schema":1,"documents":[...]}`. Each document has
`id`, repository-relative `path`, original PDF `sha256`, `pages:1`,
`selected_pages:[0]`, and `oracles:[{"page":0,"dpi":72,"path":...,
"sha256":...,"tolerance":0}]`. The supplied file contains all concrete values.
No PDF or oracle regeneration is required. Input paths remain under `--corpus`;
the harness snapshots PDFs before timed execution.

## Report both internal metrics, with these boundaries

Primary engine contribution: `engine_ns = open_ns + render_ns`. Also report
`render_ns`, `open_ns`, `emit_ns`, whole-process `elapsed_ns` and process RSS.
Do not substitute whichever internal ratio looks better. `engine_ns` is more
robust to engines moving eager work between open and page preparation.

| Engine | Open stage | Render stage |
|---|---|---|
| Pagura | Input read/open, render-context creation, page-count enumeration | `document.page`, compile and paint through `RenderContext::render_page`, including lazy resources/decoding and output allocation |
| PDFium | `PdfDocument` and page count, after Python/backend import | Page indexing/load and `selected.render`, including bitmap setup and lazy work |
| MuPDF | `backend.open`, authentication and page count, after Python/backend import | Page indexing/load and `get_pixmap`, including lazy decoding and pixmap setup |
| PDF.js | Input read, `getDocument`/loading promise and page count, after Node/module import | `getPage`, viewport/canvas creation, render promise and one-pixel readback to flush deferred playback |
| Hayro | File read, `Pdf::new_with_password`, `pdf.pages` | Settings, page lookup, first `RenderCache` creation and `hayro::render` |
| Quartz | `CGPDFDocument`, unlock and page count | Page lookup, crop/rotation geometry, bitmap/context allocation, white fill, transform and `drawPDFPage` |
| Poppler | Unavailable | Unavailable: current `pdftoppm` adapter has no internal stage timer |

Emit includes pixel extraction/conversion, raw encoding and file writing; these
are excluded from `engine_ns`/`render_ns`. Pagura also recycles its output during
emit. Stage totals exclude most runtime imports/startup and teardown, which
remain in process time. Lazy JIT, image/font loading or deferred initialization
that occurs inside open/render remains timed there. Exact placement of semantic
preparation is engine-specific; these are equivalent first-conversion tasks,
not identical internal pipelines or already-prepared compositor microbenchmarks.
Every pair starts a fresh process. Warmups warm filesystem/runtime OS effects,
not an in-process document or display-list cache. Process RSS includes the
runtime/adapter and is not renderer-only heap usage.

## Independent audit, after completion

1. Independently pin the completed outer runner receipt hash and verify its
   final report binding, zero exits, unchanged frozen source and identities.
   Require artifact-store completion. An incomplete receipt, missing identity,
   output, oracle or case is explicit missing evidence, never a pass.
2. Compare the report matrix to this external manifest: all 19 IDs, six
   competitors, page zero, 72 DPI, Exact/default, white background, pairs -2,-1
   and 0..4, valid randomized pair order. Retain every failure/unavailable cell
   and both warmup and measured outputs. Do not use report-provided selection
   or success-only filtering as the independent expected matrix.
3. Revalidate each per-execution receipt/context, command, engine identity, PDF
   snapshot and raw-output hash using the existing artifact verifier, with the
   expected contexts authenticated by the externally pinned outer receipt.
   The full generic corpus parser deliberately does not carry generated
   `render_references` into case objects. Do not claim that automatic formula
   profile checks survived this mapping; use `references.json` separately.
4. For each expected case, authenticate its frozen copied oracle against the
   original `oracle_sha256` in `references.json`. Parse its dimensions and RGB
   payload independently of every measured output. Re-read every retained raw
   output, verify its hash/format/dimensions, normalize under the pinned
   RGB-over-white policy, and compare every RGB channel to this immutable
   oracle with the original zero tolerance. Audit raw and normalized hashes,
   dimension mismatches, differing channels and maximum errors, rather than
   trusting a recorded `status: ok` or comparing rivals only with Pagura.
5. For the four image-direct cases, also verify the original Exact reference
   descriptor's PDF/settings/source/policy/derivation pins against the frozen
   source state. These rational models are independently authored and import
   no renderer. They describe the declared sampling policy, not a claim that
   PDF requires every other engine to choose that same resampler. Preserve any
   competitor mismatch and report its error; do not turn it into a general PDF
   conformance judgment or silently relax the reference.
6. Recompute per-cell paired process, open+render and render summaries from the
   authenticated measurements. A failed/incorrect output suppresses a quality-
   qualified timing comparison for that cell. Existing report agreement rows
   are investigation only. Keep Poppler's stage metrics explicitly unavailable.

## Evidence limits to state in every summary

- ArtifactStore persists each bounded raw image and an execution receipt,
  including successful warmups. Identical raw bytes may share a hashed blob.
  Missing output, retention budget exhaustion or evidence errors make the store
  incomplete; they are not permission to omit the failure.
- Hayro, PDF.js and Quartz raw output retains RGBA alpha. Pagura, PDFium,
  MuPDF and Poppler emit RGB PPM in this track, so their alpha is unavailable.
  The common oracle checks RGB over white. It proves no independent alpha
  superiority, including for the soft-mask fixture.
- Successful raw stdout/stderr is not retained by the current harness.
  The temporary combined log is bounded/decoded, parsed stage fields are stored,
  and failures retain a diagnostic tail. Stage validation rejects malformed or
  impossible records, but the original successful log cannot be reconstructed.
  Do not claim complete raw diagnostic retention or independently reparsed
  success timing logs in this first run.
- Stage fields answer fresh engine/page rendering cost without wrapper startup;
  they do not establish a same-binary warm compositor comparison. Pagura's
  existing `microbench render-warm` recompiles through the context on each call
  and is explicitly Pagura-only. Future warm adapters need a separately frozen
  repeated-render operation, not fabricated cold/warm fields in this report.
- No general PDF, release, dedicated-runner, alpha or diagnostic superiority
  claim follows from this pilot. All original performance gates stay unchanged.

## Executable post-run audit

`audit.py` reads this package's frozen manifest, references and oracle copies.
It executes no renderer or adapter. The coordinator should first run the focused
synthetic checks, then run the auditor only after the campaign's terminal receipt
has been independently pinned. Keep these files together when copying them.
Run with `PYTHONDONTWRITEBYTECODE=1` to avoid creating bytecode in the repository.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s benchmarks/results/sources/analytic19-conversion -p test_audit.py
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/results/sources/analytic19-conversion/audit.py \
  --source-root /Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura \
  --report /absolute/path/to/completed-report.json \
  --runner-receipt /absolute/path/to/terminal-runner-receipt.json \
  --runner-receipt-sha256 INDEPENDENTLY_RECORDED_RECEIPT_SHA256 \
  --output /absolute/path/to/new-audit.json
```

Exit 0 means every expected comparison satisfies this frozen RGB policy and
its evidence bindings; 1 means authenticated failures, unavailable engines or
policy mismatches remain; 2 means invalid/incomplete evidence. No exit grants
release admission. A malformed initial report, unavailable helper import or
unwritable destination fails nonzero before an audit report can be written.
The destination must be new; the auditor never overwrites previous evidence.

Image parsing, white compositing, comparisons and point estimates are separate
from the benchmark implementations. Source/receipt/identity authentication uses
the existing frozen repository helpers; it is not an independently implemented
attestation system. The auditor records its own file and Python executable pins,
the source digest, and the independently supplied outer receipt pin. Standard
library and OS authenticity are outside this local evidence model.

Every recorded warmup and sample in a structurally valid cell is classified.
Unavailable cells, missing cells, unrecorded pair counts and prelaunch refusals
stay explicit. Malformed structural records fail closed; their untrusted nested
outputs are not presented as verified attempts. Missing/malformed output receipts
remain visible even though the normal success-only artifact verifier rejects
them. Failed processes cannot become successful because a partial image matches.
Artifact incompleteness or any global binding error suppresses all quality-
qualified ratios. All five measured pairs must be present for a metric's point
estimates; failed outputs' valid process timings remain in the unqualified
distributions. No successful-only timing subset is substituted.

## Source anchors

Paths below are relative to the active checkout named above.

- `tools/benchkit/corpus.py:73-128`: concrete corpus schema and oracle pins.
- `tools/benchkit/conversion.py:17-105`: process-per-page pairs, stages and status.
- `tools/benchkit/timings.py:13-59`: strict stage record accounting.
- `tools/benchkit/artifacts.py:257-355`: context freeze and raw image retention.
- `tools/benchkit/measurement.py:154-173`: diagnostic retention limitation.
- `benchmarks/adapters/python_engine.py:36-78`, `pdfjs.mjs:28-63`,
  `hayro/src/main.rs:35-89`, `quartz.swift:24-77`: adapter boundaries.
- `crates/pagura/src/main.rs:353-383`: Pagura conversion stage boundaries.
- `crates/pagura-render/src/context.rs:170-209`: render includes compilation.

## Pins

- Original selector: `892a8a2e65bb2da08391963a1d060b43623edfcda0443b21cec3dae927fb5107`
- `manifest.json`: `278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3`
- `references.json`: `924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf`
- Each copied oracle's SHA-256 is the corresponding original `oracle_sha256`
  inside `references.json`; all 19 copied-file hashes were inspected and match.
