# Performance contract and executable harness

The release target is at least 20% lower latency in every declared applicable
comparison, with correct equivalent output and no peak-memory regression. It is
not an achieved result. `policy.json` and `release-matrix.json` retain ADR 0002's
requirements: at least 100 randomized pairs, five warmups, paired bootstrap
one-sided 95% upper bounds <= 0.80 for both median and p95, no missing cells,
and verified outputs. A fast average cannot hide a slow or incorrect cell.
Unbound workloads, missing engines, unattested runners and unimplemented
cross-platform/resolution/concurrency aggregation still block release.

## Offline checks and explicit competitor setup

```sh
python3 tools/test_bench.py
python3 tools/bench.py doctor
# Separate, explicitly authorized network operation; never part of checks:
python3 tools/competitors.py setup
```

All competitor installations live outside the repository and active linked worktree:
`$PAGURA_COMPETITORS_DIR`, default `~/.cache/pagura-competitors`. Repository
ancestors, descendants and symlink aliases into the repository are rejected.
The hayro adapter and checked-in lock are copied there; its isolated `CARGO_HOME`
and target directory are also there. It uses the workspace's pinned toolchain.
Node resolves packages from this same external cache. Dependency locks remain in
`benchmarks/`. No competitor source, tests or history are needed or read.

Set `PAGURA_POPPLER` to the actual `pdftoppm` binary when PATH supplies a launcher.
Poppler is supplied by the operating system; Adobe remains explicitly unavailable
without a licensed adapter. Doctor probes versions, reports missing executables,
and explains setup. It does not install anything. Python engine versions include
wrapper and backend; PDF.js records Node and Canvas versions; artifact hashes
cover executables, adapters, locks and discoverable backend libraries.

## Stage-timed conversion (ADR 0007)

```sh
# Independently oracled generated cases at their declared 72 DPI:
python3 tools/bench.py convert --engines pagura,pdfium,mupdf,pdfjs,hayro,quartz,poppler
# Whole documents at several DPIs (default document mode):
python3 tools/bench.py convert --corpus /path/to/private-corpus --pages all --dpis 72,144,300
# Explicit fresh-process-per-page track, allowing page subsets:
python3 tools/bench.py convert --process-per-page --corpus /path/to/private-corpus --pages 2-4,7 --dpis 144 --samples 100 --warmups 5
```

Each adapter emits one JSON timing record, using a monotonic clock, in the same
schema as `pagura convert --timings INPUT OUTDIR DPI`:

```json
{"schema":1,"open_ns":100,"pages":[{"page":0,"render_ns":200,"emit_ns":300},{"page":1,"render_ns":250,"emit_ns":350}],"total_ns":1200}
```

All adapters use these stage boundaries:

- `open`: load the file, parse it and build the page list. Obtaining the page
  count/indices or an engine's document-level page metadata belongs here.
- `render`: load the selected page and rasterize it into an in-memory image.
  Lazy page/resource loading, viewport/context setup and rasterization belong here.
- `emit`: convert/extract image pixels, encode the raw format and write it.

The default **document mode** launches one process per document, DPI and engine
in each randomized pair. That process opens once and converts all selected pages.
Document engine contribution is **open + sum(page render)**; `emit`, runtime/import
startup, teardown and process time are reported separately. Open time, process
elapsed time and RSS appear only in `document_cells`; they are never duplicated
across page samples. `cells` retains each page's verification, render and emit
samples/statistics. A missing or incorrect page invalidates document ratios.
Malformed, duplicate, negative, noninteger, wrong-page, missing-page or impossible
stage records fail the process; their pages cannot masquerade as valid samples.

Adapters support `INPUT OUTDIR DPI FIRST LAST` (zero-based inclusive indices),
writing `page-NNNN.ppm`; raw RGBA8 can use the same extension because normalization
reads the header. The single-page `INPUT OUTPUT DPI PAGE` form stays available.
Poppler uses `pdftoppm -r DPI -f FIRST -l LAST INPUT PREFIX` (one-based indices),
with no `-singlefile`. Its numeric output suffixes are mapped to page indices;
missing, extra or duplicate page files fail verification. Poppler has no internal
timers: stages remain unavailable and only document process time is compared.

**Pagura document comparisons use whole documents or contiguous ranges**
(`pagura convert INPUT OUTDIR DPI FIRST LAST`, zero-based). Standalone competitor command construction supports
contiguous ranges; disjoint ranges require separate processes and are rejected in
document mode. `--process-per-page` explicitly restores the original page subset
track: both engines reopen for every selected page, with a separate label and
statistics. It is also the existing default for `regress`; regression behaviour
has otherwise stayed the same.

Document/DPI jobs and within-pair engine order are seeded and randomized.
OS filesystem caches warm after warmups; this is not a cold-storage test.
`elapsed_ns` includes common system-time instrumentation.
RSS is per child, not cumulative `getrusage` across previously finished children.
A failing instrumentation wrapper fails the sample; missing RSS cannot pass the
gate. Timeouts terminate the process group on POSIX.

Adapters emit P6 PPM (Pagura, PDFium, MuPDF, Poppler) or packed RGBA8 (PDF.js,
hayro, CoreGraphics). The harness also accepts RGB/RGB_ALPHA P7 PAM. RGBA8 has an
ASCII header `RGBA8 WIDTH HEIGHT straight\n` or
`RGBA8 WIDTH HEIGHT premultiplied\n`, then tightly packed top-to-bottom bytes.
PAM RGBA is straight alpha. Normalization composites on white outside all timed
regions, with integer round-to-nearest for straight alpha. Premultiplied channels
must be <= alpha. Readers enforce dimensions, byte budgets and exact payload
lengths; they never trim pixel whitespace or resize output to match an oracle.

### Arbitrary corpora and correctness

`--corpus` accepts any local directory with `manifest.json` (or an explicit
`--manifest` path). No corpus is downloaded or uploaded. Example:

```json
{
  "schema": 1,
  "documents": [{
    "id": "example",
    "path": "example.pdf",
    "sha256": "<SHA-256 of PDF bytes>",
    "pages": 3,
    "oracles": [{
      "page": 0, "dpi": 72,
      "path": "independent-page-1.ppm",
      "sha256": "<SHA-256 of independent image bytes>"
    }]
  }]
}
```

Manifest/oracle page indices are zero-based; CLI page selections are one-based.
Paths are relative to the corpus and must resolve within it. Every input and
oracle hash is checked; duplicate document IDs/oracle cells and out-of-range
pages fail validation. The operator supplies page counts and independent oracles;
the page count itself is metadata, not a conformance expectation. Oracles must
come from independent analysis, never engine output or majority voting. Generated
oracles apply only at their declared DPI; other DPIs become unverified.

Every selected page output is validated after the complete process exits. Only a
bounded pair of normalized page images is held for agreement; large documents
retain raw pages on temporary disk instead of keeping all rasters in memory. A matching independent oracle
is `ok`; a mismatch is `incorrect`. No oracle means **unverified**, even when
all engines agree. Per-pair mean/max differences and per-channel metrics are
investigation data. The report also compares every available engine pair using
each engine's first successfully decoded measured output, explicitly identifying
the pair indices and pixel hashes. This representative comparison does not imply
that all outputs across the run are identical. Dimensions that differ remain
incomparable. No tolerance, blur mask, realignment or resizing hides differences.

All measured pairs and warmup failures remain in the report. Any failed measured
output suppresses performance ratios; invalid output is never dropped to obtain
a faster ratio. Unverified pairs can have exploratory performance statistics,
with the same paired median/p95 bootstrap as the existing end-to-end track. Document
statistics include open, summed render/emit, engine contribution and process time;
page statistics in document mode include render/emit alone. These statistics
**never feed release acceptance**. Conversion and regression reports are always
release-ineligible, including independently oracled cells.

## Pagura regression track

```sh
python3 tools/bench.py regress --revision HEAD~1 --corpus /path/to/private-corpus --dpis 72,144
```

The working tree builds offline in release mode. The specified trusted Git
revision resolves to an exact commit, is checked out detached into a temporary
worktree outside the repository, and builds offline using the current pinned
workspace toolchain and a separate temporary target directory. The revision's
build files are trusted build input, not PDF instructions. Both binaries use
`render --timings`, identical page/DPI jobs, randomized paired order and the same
statistics/verification policy. The report records the exact revision, executable
hash and version. Temporary worktree/target cleanup runs on success and build/run
failure. The operator must allow normal Git worktree metadata writes; the unit tests
exercise this lifecycle in disposable repositories only. Old revisions
without `render --timings` fail explicitly; there is no approximate fallback.

## Reports, summaries and the existing gate

Raw samples/checkpoints are restricted to ignored `benchmarks/results/local/`:
`conversion.json`, `regress.json`, or an explicit `--output` there. Bounded raw failure images are retained there with hashed filenames. Reports contain
versions/hashes, input and source digests, settings, order, stage and process
samples, per-process RSS, errors and verification status. Adjacent Markdown is a
local preview. No raw benchmark samples should be committed.

```sh
python3 tools/bench.py summary benchmarks/results/local/conversion.json benchmarks/results/my-reviewed-summary.md
python3 tools/bench.py gate benchmarks/results/local/conversion.json
# Legacy analytic bootstrap process track remains available:
python3 tools/bench.py run --suite smoke --samples 10 --warmups 3
```

The summary command creates a new Markdown file with input, source, policy and
raw-report digests. It never overwrites existing summaries. Review summaries
before committing, especially when IDs reveal private corpus information. The
gate exits 2 when blocked. Smoke, conversion and regression evidence cannot pass
the release gate; per-cell acceptance never overrides release scope.

The legacy `render-e2e` track times executable launch through raw-output write
and teardown, with exact synthetic oracles outside the timed region. Existing
committed bootstrap summaries describe older measurements and their original
boundaries; they have not been reclassified as conversion evidence.

## Remaining release work

Real competitors must be run after integration; offline fake engines verify the
harness rather than competitor API compatibility. Add equal-work persistent
open/warm/tile/text/save tracks, reviewed per-feature real-document oracles and
colour/annotation/box policies, Adobe access, fixed concurrency, multiple-platform
attestations and aggregation. A release protocol also needs pre-registered
replication and multiple-comparison handling; per-cell confidence bounds are not
a simultaneous family-wise claim. No correctness or 20% superiority claim is
made by this implementation.

## Harness modules

`tools/bench.py` is the CLI. Implementation lives in `tools/benchkit/`: `engines`
(commands, versions, historical builds), `measurement` (processes/RSS/timeouts),
`timings` (strict stage records), `images` (normalization and agreement), `corpus`
(manifests), `statistics` and `gate` (existing bootstrap and release rules),
`reports` (provenance/summaries), `documents` (one-open document conversions),
`conversion` (track dispatch and page mode), and `legacy` (original process track).
The former `benchlib.py` implementation is merged into these modules. Tests
import the package directly and retain all earlier negative controls.
