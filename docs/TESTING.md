# Testing contract

Tests are layered: object syntax/limits; document structure; independent fixture
generation; analytic raster expectations; cross-engine rendering; save/reopen;
profile validation; deterministic mutation tests; adversarial resource tests;
and performance evidence. No single renderer or validator is a complete oracle.

`python3 tools/check.py` runs formatting, architecture/safety policy checks,
Clippy, Rust tests, Python harness tests, fixture regeneration checks, and
CLI integration tests. It needs no network and cannot refresh golden outputs.

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
for sustained coverage-guided fuzzing. Future fuzz tooling stays outside product
dependencies and must preserve the safe-Rust product boundary.

The complete performance gate is tested with negative cases: missing engines,
incomplete samples, incorrect output, incomparable configs, empty matrices,
fabricated quality status, stale identities and insufficient speed improvements.
