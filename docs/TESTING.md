# Testing contract

Tests are layered: object syntax/limits; document structure; independent fixture
generation; analytic raster expectations; cross-engine rendering; save/reopen;
profile validation; deterministic mutation tests; adversarial resource tests;
and performance evidence. No single renderer or validator is a complete oracle.

The proposed [renderer-unification gates](plans/unified-renderer-validation.md)
distinguish independent references, internal parity, and release evidence, and
specify additional tests and harness work. Initial tooling is recorded in the
[implementation log](plans/unified-renderer-progress.md); the complete gates
are not yet implemented.

`python3 tools/check.py` runs formatting, architecture/safety policy checks,
Clippy, Rust tests, Python harness tests, fixture regeneration checks, and
CLI integration tests. It needs no network and cannot refresh golden outputs.

`python3 tools/check.py --jobs 1 --dry-run` prints the planned commands and
environment overrides without running any checks. On an exclusively allocated
runner, `python3 tools/check.py --jobs 1` limits Cargo build jobs, Rust test
harness threads, and conformance workers. Commands remain sequential. The flag
does not reserve a runner or control every thread: host/browser checks can still
create worker threads. Omitting `--jobs` preserves existing defaults and inherited
concurrency settings. Do not start this workflow alongside another active local
build, conformance run, profile, or benchmark.

During an agreed local window, use the [runner wrapper](runner-coordination.md)
to hold both cooperative locks around that command and retain a source-bound
receipt. Its locks do not establish dedicated-runner conditions. Benchmark
reports now retain raw artifacts and warmups; `bench.py gate` requires a final
runner receipt and its independently obtained SHA-256 pin. Missing, changed or
incomplete artifacts block admission. The four reviewed image-direct references
are covered by the [narrow rendering policy](references/image-direct-policy.md);
they do not establish a reference model for every fixture family.

Synthetic PDFs are generated independently in Python (`tools/fixturegen/<area>.py`,
standard library only, never importing Pagura) with known structure and geometry.
Each area writes `corpus/generated/<area>/` and `corpus/manifests/<area>.json`. Product writer round-trips are tested separately,
so a parser and writer cannot conceal a shared bug. Test requirement IDs live in
`references/requirements/<area>.json`; this is a coverage ledger, not certification.

Imported corpora are pinned by Git revision and file hashes. Metadata records
licenses and authoritative source. Cases with no established oracle are
investigations, not conformance passes. The PDF Differences README screenshots
are illustrative and must never be used as exact golden images.

Rendering comparisons use identical page, CropBox/MediaBox choice, scale, rotation,
background, colorspace, annotation/layer state, and fonts. Exact PPM equality is
appropriate only for the aligned, opaque bootstrap fixtures. Future cross-engine
tests need localized metrics, geometry/text assertions and reviewed tolerances.
Never raise a threshold merely to make a regression pass.

A no-op round trip must preserve original bytes. Content edits require semantic,
structural and visual checks. Redaction additionally checks old revisions,
metadata, hidden layers, attachments, resources and extracted text. Signatures
require cryptographic, revision and permissions checks, not visual comparisons.

The mutation smoke test is deterministic and reproducible; it is not a substitute
for coverage-guided fuzzing. That lives in the separate `fuzz/` workspace, the
only place `no_mangle` entry points exist, and never becomes a product
dependency. `python3 tools/fuzz_smoke.py` runs every target (syntax, filters,
document, render, font, image, text, content, host, navigation, extraction)
for a short bounded time on nightly Rust. Every input fuzzing finds is
minimized into `fuzz/regressions/` and replayed on stable Rust by
`crates/pagura/tests/fuzz_regressions.rs`, which `tools/check.py` runs. Its
known-failures list is the only exception, and it fails once an entry is
fixed, so fixed entries are removed.

The complete performance gate is tested with negative cases: missing engines,
incomplete samples, incorrect output, incomparable configs, empty matrices,
fabricated quality status, stale identities and insufficient speed improvements.
