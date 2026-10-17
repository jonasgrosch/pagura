# Test corpus

Three corpora (ADR 0007):

- **generated** — `generated/<area>/` holds original, deterministic fixtures
  from `tools/fixturegen/<area>.py` with independent expectations;
  `manifests/<area>.json` records hashes, expected outcomes and requirement
  IDs. `tools/fixtures.py --check` detects drift without blessing it, and
  `tools/conformance.py` checks every PDF case against its expectation.
- **upstream** — public test suites and other projects' own test PDFs
  (veraPDF, PDF Association suites, pdf.js, PDFium, qpdf, PDFBox, Poppler),
  fetched by `tools/corpus.py` as shallow, blob-less sparse checkouts of PDFs
  and licence notices only, pinned per source in `upstream/<id>.lock.json`
  and stored in `$PAGURA_CORPORA_DIR`. They are input documents: their
  expected outputs are never oracles.
- **realworld** — one pinned zip (about 1,000 PDFs) of the SafeDocs
  CC-MAIN-2021-31 corpus from Digital Corpora, crawled from the public web in
  2021 and subject to Common Crawl's terms of use. `tools/zipcorpus.py fetch`
  downloads it into `$PAGURA_CORPORA_DIR` (outside the repository), verifies
  it against `realworld.lock.json` and extracts only `NNNNNNN.pdf` members.
  These files have no oracles; they feed the scoreboard
  (`tools/scoreboard.py "$(python3 tools/zipcorpus.py path realworld)" --name realworld`),
  which is the M1 metric. Never commit or redistribute them.
- **adversarial** — one pinned zip (1,416 files) of the SafeDocs
  CC-MAIN-2021-31-UNSAFE `corpora-pdf` collection: PDFs mutated by fuzzers and
  grammar-aware tools to crash or hang parsers. `tools/zipcorpus.py fetch
  adversarial` pins and extracts it; the scoreboard must show no crashes and
  no timeouts.

`upstream.lock.json` records pinned official test-data/model repositories:
PDF Association PDF 2.0 examples, PDF Differences, Arlington, and veraPDF corpus.
`tools/corpus.py fetch` acquires a bounded deterministic sample of eligible data
plus available license notices into `.cache/corpora/`. It reuses recorded Git
revisions; changing them requires `--refresh`. All selected Git blobs are verified
and SHA-256 recorded. Acquisition defaults to 24 data files per source, 20 MiB per
file and 100 MiB overall. The ledger reports eligible versus acquired counts.

These samples are **not representative release coverage**, and they are not yet
mapped to executable conformance assertions. Files without independently reviewed
expected behavior do not become golden images automatically. Arlington describes
PDF objects but does not replace normative text. veraPDF fixtures cover profile
validation, not every rendering requirement. PDF Differences shows disagreements,
not a single universally correct renderer. Ghent access is indexed separately.

Private/customer documents belong in ignored `corpus/private/` with provenance
and permission records; never upload them to benchmark services. Public download
availability does not imply redistribution permission.
