# Performance contract and executable harness

The release target is at least 20% lower latency in every declared applicable
comparison, with correct equivalent output and no peak-memory regression. It is
not an achieved result. `policy.json` and `release-matrix.json` retain ADR 0002's
requirements: at least 100 randomized pairs, five warmups, paired bootstrap
one-sided 95% upper bounds <= 0.80 for both median and p95, no missing cells,
and verified outputs. A fast average cannot hide a slow or incorrect cell.
Unbound workloads, missing engines, unattested runners and unimplemented
cross-platform/resolution/concurrency aggregation still block release.

## Capability inventory (G00)

`capabilities.json` is the reviewed source-inspection ledger for the
[competitive plan](../docs/COMPETITIVE_PARITY.md). Its generated
[review view](../docs/CAPABILITIES.md) separates library, CLI, host and viewer
support. All 90 literal tags have manual dispositions with hashed call-site evidence.
The 43 capability families own all 332 registered requirements and include untagged APIs.
Every baseline family cell is explicitly unprobed; schema 2 rejects runtime support
or N/A claims. G01 fixture-level results and build identities are recorded separately.

```sh
uv run --locked --offline python tools/capabilities.py --check
# After reviewing ledger edits, regenerate the checked view:
uv run --locked --offline python tools/capabilities.py
```

The check validates IDs, owners, packages, fixture/requirement references,
LF-normalized source hashes, tag dispositions/locators, requirement/API ownership
and the generated view.
Regeneration never refreshes source hashes or reclassifies a tag automatically.
When source evidence changes, review the affected observations before updating
their hashes. Tests/fixtures are references to existing evidence, not new run
results. The scan is lexical; the manual review followed symbol uses, aliases
and decoder dispatch. New dynamic tags or APIs reopen the inventory.
No competitor download, engine invocation or product behavior change occurs.

G01's first [document-service probes](probes/README.md) exercise the pinned
PDFium/MuPDF APIs on independent synthetic controls in hosted CI. Results remain
fixture-scoped; the remaining baseline adapters stay explicit and unprobed.

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
The Python engines are the `bench` dependency group in `pyproject.toml`; setup
runs `uv sync --locked --only-group bench` into `venv/` there, so `uv.lock` pins
every transitive wheel by hash and `.python-version` pins the interpreter.
The hayro adapter and checked-in lock are copied there; its isolated `CARGO_HOME`
and target directory are also there. It uses the workspace's pinned toolchain
and the native target flags `.cargo/config.toml` gives the host, resolved as
`round5.py` resolves Pagura's; `hayro-build.json` there records them, and hayro's
engine identity reports them as `build`.
Node resolves packages from this same external cache. Dependency locks remain in
`benchmarks/`. Studying other engines is allowed; copying or translating their
implementation remains forbidden (ADR 0006).

Set `PAGURA_POPPLER` to the actual `pdftoppm` binary when PATH supplies a launcher.
Poppler is supplied by the operating system. Adobe is not a required baseline
(ADR 0011). Doctor probes versions, reports missing executables,
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

PDF.js forces deferred Canvas raster playback with a one-pixel read before the
render timer stops; full image extraction stays in emit. Its synchronous raster
Canvas path uses no async image encoder. Runtime background work is still checked
through process CPU/wall rather than assumed absent from the process.

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

Pagura's integrated CLI now supports zero-based inclusive conversion ranges:
`pagura convert --timings INPUT OUTDIR DPI FIRST LAST`. Document mode forwards
exactly the selected range to every engine. Disjoint ranges require separate
processes and remain rejected in document mode. `--process-per-page` restores the
original per-page track; it is the existing default for `regress`.

Document/DPI jobs and within-pair engine order are seeded and randomized.
OS filesystem caches warm after warmups; this is not a cold-storage test.
`elapsed_ns` includes common system-time instrumentation.
RSS is per child, not cumulative `getrusage` across previously finished children.
A failing instrumentation wrapper fails the sample; missing RSS cannot pass the
gate. `--no-rss` explicitly omits unavailable instrumentation in exploratory
conversion/regression runs and records `rss_enabled: false` and null RSS.
It does not change checked-in policy or make the release gate eligible. Timeouts terminate the process group on POSIX.

### Exploratory CPU accounting and WebAssembly

`python3 benchmarks/process_measurement.py convert ...` is an opt-in POSIX front
end to the same CLI. It replaces only process instrumentation with `wait4`:
each raw document sample retains `cpu_user_ns`, `cpu_system_ns`, their sum
`cpu_ns`, wall `elapsed_ns`, `cpu_wall_ratio`, and `cpu_parallelism_flag` (>1.05).
CPU accounting starts at zero for each child and is read after that child exits;
it is never a cumulative delta across unrelated children. Engine launch/runtime startup
is included in process CPU and wall, separately from engine stages. A ratio below
1.05 does not prove that no worker ever ran. Waited-for descendants contribute
CPU; RSS is an individual-process high-water mark, not a summed process tree.
On Linux, a small `python3 -I -S` supervisor starts the engine with
`os.posix_spawnp` and reports that engine child's own `wait4` CPU, RSS and exact
exit status in a fixed plain-text record. It times just before the engine is
spawned through the return of `wait4`. Its isolated address space
sets an RSS floor measured separately for each run. The supervisor's inherited
`maxrss` is never used: Linux retains the old address space's high-water mark
across exec, including `posix_spawn` and shell exec. No GNU `time` installation
is needed; the runner's existing Python and `benchmarks/linux_process.py` suffice.
The macOS path continues to read the directly launched engine's `wait4` usage.
Timeouts kill the supervisor's entire process group, including the engine and
its descendants. A killed Linux supervisor cannot return the engine's final
accounting, so timeout samples retain wall time and the negative signal status
with CPU/RSS explicitly unavailable. Missing or invalid accounting fails the
sample; it never falls back to the supervisor's inflated RSS. Absent `wait4`
fails explicitly. Non-timeout samples use the supervisor's engine wall/CPU
values, excluding the supervisor's Python startup, imports, accounting
serialization and exit.
Timeouts retain the outer wall time. Engine stage timers are unchanged.

`adapters/wasm_convert.rs` calls the same public facade and rendering function as
`pagura convert`. It is built against the native and `wasm32-wasip1` release
libraries with identical `bundled-data` features. The runner also builds the
product WASI host. Render is exactly page lookup plus `RenderContext::render_page`
(compilation and painting); RGB conversion and writes belong to emit. There is
no session framing or RGBA packing in render. A native build of the adapter is
an analytic test control, not an extra measured competitor.

`adapters/wasi.mjs` runs that conversion adapter in Node/V8 WASI preview1, with
only input/output directories preopened. V8 uses `--single-threaded --no-liftoff
--no-wasm-lazy-compilation --wasm-num-compilation-tasks=1`: optimizing compilation of
the whole module happens at instantiation, before Rust stage timers, and remains included
in process CPU/wall/RSS. Without `--no-wasm-lazy-compilation`, V8 compiles each function
on its first call, inside the timed stages; runs before 2026-10-19 measured that way,
which added about 0.7–1 s to the timed stages and made V8 appear 3–10× slower than
native instead of about 1.4–1.9×. Process wall time now includes compiling the whole
module instead, so V8's process ratio rises while its stage ratio falls. Compare V8
cells only within one setting; `summary.md` repeats this note whenever V8 is timed. Wasmtime
49.0.3 uses a locally generated, hash-recorded Cranelift AOT artifact (optimization
level 2, parallel compilation disabled). Never load an untrusted AOT artifact.
The native and WASM builds use the repository's `.cargo/config.toml` flags for
their target, as `cargo build` in the repository does: native code relies on
compiler autovectorization, and WASM gets SIMD128. The harness resolves the
flags itself and passes them through `CARGO_ENCODED_RUSTFLAGS`, so inherited
`RUSTFLAGS` and user-level Cargo configuration never reach the build.
`compiler.json` records the flags (`native_rustflags`, `wasm_rustflags`) and the
compiler's enabled target features, such as NEON on aarch64 and SSE2/SSE4.1
(without AVX2 or AVX-512) for x86-64-v2. Other engines' ISA dispatch is not audited.

The optional `round5.py --pagura-x86-64-v3` track builds a separate
`pagura-x86-64-v3` binary with `RUSTFLAGS="-C target-cpu=x86-64-v3"` in an
isolated target directory. It appears under that name in its own comparison
rows; headline `pagura` remains the paired reference. The optional
track must match the headline build's pixels exactly in every block, including
warmups, before ratios can be reported. It is reported unavailable on other
architectures or x86 hosts missing the required instructions. Compiler-enabled
target features, host feature checks and flags are retained in `compiler.json`
and `machine.json`. Competitor tuning is unchanged; these exploratory rows do
not establish an equivalently tuned release comparison. The track is a Pagura
build: with `--engines` it requires `pagura` and is timed alongside it, and with
`--reference` it is re-timed against stored competitor medians but never counts
as the best competitor.

Every warmup and measured WASI output must be byte-identical to its block's
native Pagura pixels. A mismatch is retained as `runtime-parity-mismatch` and
suppresses ratios; native output is a runtime parity reference, not a PDF oracle.
Independent analytic two-page fixtures check both runtimes and the native control
at 72/144 dpi, including out-of-range refusal. Poppler remains process-time only.
PDF.js uses its Node main-thread path, synchronous raster Canvas/readback, and no
async encoder; V8 background tasks are disabled. Native library background work
cannot be ruled out from that API alone: every CPU/wall flag remains visible.

### One unattended round-5 entry point

Prepare a macOS or Linux runner (aarch64 or x86_64) with uv 0.13.1+ (it
provides the Python pinned in `.python-version`), the
repository's pinned Rust 1.99.1 toolchain including `wasm32-wasip1`, Node 26.8.1,
and the operating system's Poppler. The actual Poppler version and executable
hash are recorded per runner in `engines.json`; no Poppler version is enforced.
macOS additionally needs Xcode command-line tools (Swift/CoreGraphics).
Install competitors **outside the repository** with the explicitly networked
`python3 tools/competitors.py setup`
before running. The checked-in Python (`uv.lock`), Node and Cargo locks pin pypdfium2 5.14.0
(PDFium 156), PyMuPDF 2.0 (MuPDF 1.28.5), Pillow 13.0.0, pdf.js 6.4.299, Canvas 1.0.11 and
hayro 0.8.0. Supply Wasmtime 49.0.3 separately, verifying its downloaded archive
against the release's published SHA-256. No setup/download occurs during a run.
Keep at least 15 GB free for build artifacts and representative PPMs.

Set these environment variables (absolute paths; the manifest stays external):

| Variable | Meaning |
|---|---|
| `PAGURA_CORPUS_DIR` | Corpus root matching the manifest's relative PDF paths |
| `PAGURA_BENCH_MANIFEST` | Original external round-4 40-page manifest |
| `PAGURA_BENCH_MANIFEST_SHA256` | Trusted SHA-256 of that exact manifest |
| `PAGURA_COMPETITORS_DIR` | External cache populated by competitor setup |
| `PAGURA_WASMTIME` | Wasmtime 49.0.3 executable |
| `PAGURA_WASMTIME_SHA256` | Optional trusted executable digest, checked in addition to recording its actual digest |
| `PAGURA_POPPLER` | Actual `pdftoppm` executable, or supply it on PATH |
| `PAGURA_QUARTZ`, `PAGURA_HAYRO` | Optional external rebuilt adapter overrides |
| `PAGURA_NODE` | Optional Node executable override; otherwise PATH |

From a clean checkout, after setting that environment, run exactly:

```sh
"$PAGURA_COMPETITORS_DIR/venv/bin/python" benchmarks/round5.py \
  --output benchmarks/results/local/round5 --dpi300-pages 20
```

The entry point builds Pagura and its WASI conversion adapter offline in a fresh
output-local target, checks every input hash and engine availability, and runs
the independent native/WASI analytic controls before measurement. macOS also
runs the CoreGraphics geometry control; CoreGraphics is omitted on Linux.
It uses all 40 pages at 72 and 150 dpi. To bound the unattended
round's two-hour budget, the command predeclares 20 pages at 300 dpi: five per
content category, 4 small/13 standard/3 large, from 20 distinct files/producers.
Selection is seeded and independent of timings, and never enlarges the existing
16-million-pixel normalization budget. Missing strata fail explicitly. An explicit
`--dpi300-pages 40` includes every page, retaining any budget-refused cells.
The manifest is read in place and is never copied into the repository.

Document/DPI jobs and all applicable engines (nine on macOS, eight on Linux)
are randomized in complete paired blocks (seed 52018): one open/process per
document range/engine/block, three warmups,
ten samples. Pagura's sample is shared across comparisons, so those comparisons
are correlated. Each cell is measured once; failures and warmup failures remain.
The two-hour deadline includes build/preflight work and is checked before each
job, block and process; child timeouts are bounded by its remaining time. Partial
results are labelled incomplete. Machine/process/power observations precede each
job and finish the run. The harness and its probe descendants are excluded from
the idle check. Other processes at or above 25% CPU trigger up to three 30-second pauses, then an
explicit failure; denied process inspection is recorded as unavailable, not idle.

All artifacts are under the one new output directory: `blocks.jsonl` (every
raw block, including warmups and CPU/wall/RSS), `processes.jsonl` (every completed
process, including partial blocks), `conversion.json` (stages, paired
bootstrap bounds, parity and exact representative differences), `cpu-summary.json`
(including measured and warmup >1.05 flags and RSS peaks), `machine.json`,
`engines.json`, `compiler.json`,
`status.json`, build/preflight logs, representative PPMs, bounded `failures/`
artifacts and `summary.md`.
On Linux, `conversion.json` records the supervisor command and the measured
`/bin/true` RSS floor in `measurement_options` (`rss_launcher`,
`rss_floor_command`, `rss_floor_bytes`). This calibration runs once per matrix
run, outside measured blocks; it is reported as a floor, never subtracted from
engine peaks.
No image is resized or realigned. Different dimensions remain incomparable;
representative outliers are flagged for visual review (dimension mismatch,
mean channel error >=10/255, or max >=200 with >=2% changed channels).
Agreement does not establish correctness. Source/binary changes fail the run.
The release gate remains blocked. `status.json` distinguishes complete/failed
cells from an interrupted/incomplete run; process failure or parity failure
returns a nonzero exit status.

The only development-machine run should be the functional smoke:

```sh
"$PAGURA_COMPETITORS_DIR/venv/bin/python" benchmarks/round5.py \
  --smoke --output benchmarks/results/local/round5-smoke
```

Smoke selects one text and one image page, 72 dpi, one warmup and three samples
for every engine. It records a busy/unattested machine without waiting for idle;
its timings are never performance evidence. Offline unit controls:
`"$PAGURA_COMPETITORS_DIR/venv/bin/python" -m unittest discover -s benchmarks -p 'test_*.py'`.

Adapters emit P6 PPM (Pagura, PDFium, MuPDF, Poppler) or packed RGBA8 (PDF.js,
hayro, CoreGraphics). The harness also accepts RGB/RGB_ALPHA P7 PAM. RGBA8 has an
ASCII header `RGBA8 WIDTH HEIGHT straight\n` or
`RGBA8 WIDTH HEIGHT premultiplied\n`, then tightly packed top-to-bottom bytes.
PAM RGBA is straight alpha. Normalization composites on white outside all timed
regions, with integer round-to-nearest for straight alpha. Premultiplied channels
must be <= alpha. Readers enforce dimensions, byte budgets and exact payload
lengths; they never trim pixel whitespace or resize output to match an oracle.

### Re-time selected engines against round 5

After a change to Pagura, reuse an existing run on the same runner:

```sh
"$PAGURA_COMPETITORS_DIR/venv/bin/python" benchmarks/round5.py \
  --output benchmarks/results/local/round5-retime \
  --engines pagura --reference /absolute/path/to/round5-run --control 4
```

`--engines NAME[,NAME...]` selects distinct applicable engine names. Without any
new options the all-engine protocol and reports are unchanged. Selected engines
keep the same page/DPI plan, randomized blocks, three warmups, ten samples, idle
checks, stage boundaries and per-process CPU/RSS accounting. Unselected engines
are not launched. Pagura-only runs build the native CLI and geometry control
without building WASI or requiring Wasmtime. Competitors needed only as stored
references need not be installed in the current checkout.

`--reference RUN_DIR` reads the completed run's `conversion.json`, `blocks.jsonl`,
`machine.json`, `engines.json` and `status.json`. A different external manifest
hash, CPU model, OS/platform or architecture is refused, as are incomplete runs,
failed samples and inconsistent engine identities. References must use identical
sample and warmup counts, policy hash, measurement options (except the measured
`rss_floor_bytes`), and `process_measurement.py` hash. A re-timing run cannot be
a reference. `--reference` requires at least one control cell. The selected
page/DPI plan must be covered by the reference. Reference document ranges are preserved, so
open/process costs remain comparable. `reference.json` records the reference run
id, commit, manifest hash, machine identity and hashes of those five files;
`conversion.json` also retains that provenance. Files remain in place.

`--control N` requires a reference and re-times N fixed Hayro page/DPI cells.
The selection cycles through text, image, vector and mixed categories, rotating
72/150/300 dpi, with case/page ordering independent of performance. Each control
runs its reference's complete document range; overlapping jobs share their
blocks. The Hayro version and artifact hashes must match the reference even
when their paths move. Controls check render, engine contribution and process
elapsed time separately: per-page render medians and per-document engine/process
medians, counting each document/DPI once. Each metric's cell ratios (new ÷
reference), median ratio and status appear in `comparison.json` and `summary.md`.
All three medians must be within [0.97, 1.03] for the comparison to be stable.
Any metric outside that interval marks the comparison **DRIFTED**; missing
control results mark it incomplete. No drift correction is applied. Any status
other than stable prints **NOT A RESULT**, withholds comparison ratio tables,
and returns exit code 3. Raw measurements and control diagnostics are retained.
Controls do not replace stored competitor medians.

`medians.json` retains newly measured per-page render/emit and per-document
stage/process/CPU/RSS medians. `comparison.json` and `summary.md` compare Pagura
with every engine, the fastest applicable competitor per cell, and reference
Pagura. Best excludes Pagura's V8/Wasmtime and x86-64-v3 builds. Each category/DPI
entry is the median of its cell ratios of medians; reference values are used
only for engines absent from `--engines`. Render
cells are pages; engine contribution (`open + render`) and process cells are
document/DPI jobs, counted once per category. Poppler remains process-only.
Missing or failed results suppress ratios rather than narrowing the sample.
These cross-run comparisons are unpaired and exploratory, not release evidence.

Add `--smoke` for a functional check: one text and one image page at 72 dpi,
three samples and one warmup, plus the requested controls. A reference's full
document ranges are retained even when the smoke selects just one page from a
range. Control cells still span categories and DPIs. Smoke bypasses the idle
wait and its timings are not performance evidence. A smoke re-timing requires a
compatible smoke reference; it cannot use a full ten-sample round-5 reference.

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
A document may declare `selected_pages: [0, 3]`; these must be unique, in range
and contiguous for a single document-mode job. This permits benchmarking existing
renderable pages in place without extracting/copying a PDF. A CLI selection must
stay within the declared selection. The original total `pages` remains metadata.

Generated cases expand their actual manifest `pages` count. Their primary oracle
applies only to page 0 at its declared DPI; additional pages/DPIs stay unverified.
A descriptive `page_oracles` string is never treated as a pixel oracle.

Manifest `password` strings, including empty passwords, are passed literally to
Pagura `--password`, PDFium's constructor, MuPDF authentication, PDF.js document
loading, hayro `Pdf::new_with_password`, CoreGraphics unlocking and both Poppler
`-upw`/`-opw` roles. Authentication belongs to open. Commands and manifests in
private raw reports may contain credentials. Explicit passwords are authenticated even
when an engine auto-opens the document with an empty user password. An adapter without a password API
may emit `{ "status": "unsupported", "feature": "security.password" }` and exit
3; this is distinguished from failed authentication and remains gate-blocking.
Native adapter overrides `PAGURA_HAYRO` / `PAGURA_QUARTZ` select rebuilt executables
outside the repository; reports hash the actual selected binaries.
CoreGraphics applies DPI explicitly: its page-fitting transform only scales down.
After rebuilding that adapter, run `python3 benchmarks/adapters/quartz_geometry_test.py /external/path/to/quartz`.
This opt-in platform check requires exact independent rectangle pixels at 36, 72
and 150 dpi, both rotations and a nonzero page origin; portable checks still use
fake engines and require no competitor installation.

`tolerance` is an integer per-channel maximum absolute RGB8 difference (0..255,
default 0). Generated cases inherit it; arbitrary manifests can set a document
value or override it per oracle. Dimensions must still match exactly. Every
oracled output retains exact mean/max/channel/difference counts and pixel hashes,
even when within tolerance. Unverified output never becomes verified from a
bound or agreement. The release gate's existing exact-hash controls remain.

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
incomparable. No blur mask, realignment or resizing hides differences; declared tolerances do
not suppress exact difference metrics.

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

The summary command creates a new Markdown file (including under ignored
`benchmarks/results/local/` for private exploratory runs) with input, source, policy and
raw-report digests. It never overwrites existing summaries. Review summaries
before committing, especially when IDs reveal private corpus information. The
gate exits 2 when blocked. Smoke, conversion and regression evidence cannot pass
the release gate; per-cell acceptance never overrides release scope.

The legacy `render-e2e` track times executable launch through raw-output write
and teardown, with exact synthetic oracles outside the timed region. Existing
committed bootstrap summaries describe older measurements and their original
boundaries; they have not been reclassified as conversion evidence.

## Remaining release work

Offline fake engines verify harness contracts; actual installed adapter versions
and raw measurements must still accompany any real comparison. Add equal-work persistent
open/warm/tile/text/save tracks, reviewed per-feature real-document oracles and
colour/annotation/box policies, fixed concurrency, multiple-platform
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
