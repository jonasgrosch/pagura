# G01 capability probes

The document-service manifest compares page counts/dimensions, Info metadata,
catalog and first-page FileAttachment payloads, and text through public APIs.
It contains 21 independently authored controls and seven subjects: Pagura plus
PDFium, MuPDF, pdf.js, Poppler, hayro and CoreGraphics. The 147 observations are
fixture-specific; they do not establish full document-service or product parity.

| Subject | Public adapter | Explicit limits |
|---|---|---|
| Pagura | CLI inspect, metadata, attachments, extract-attachment, text | New AF/RF, object packets, collection and asset-source APIs have independent product tests; comparative probes remain open |
| PDFium | Locked pypdfium2 pages, metadata, catalog/page attachment and text APIs | Optional V8/XFA builds are unprobed |
| MuPDF | Locked PyMuPDF public name-tree/stream, page attachment and text APIs | Optional providers are unprobed |
| pdf.js | Locked legacy Node loading/attachment APIs | Browser variants are unprobed; dynamic evaluation is disabled |
| Poppler | GLib/GI pages, metadata, catalog/page attachments and text | Other bindings are unprobed |
| hayro | Locked public page, metadata, object and stream APIs | Attachment names are ASCII; text reconstruction is not implemented in the adapter |
| CoreGraphics | System CGPDFDocument, dictionary/stream and page APIs | Text reconstruction is not implemented; PDFKit is not substituted |

The [manifest](document-services.json) owns exact expected names, dimensions,
metadata values, text and SHA-256 payload identities. Fixtures are authored by
[capability_probes.py](../../tools/capability_probes.py) without a product or
competitor writer. Catalog and page scopes stay separate, duplicate occurrences
are preserved, and nested name-tree traversal is exercised. Filenames never
select output paths. The text contract only normalizes CRLF and outer newlines.
Metadata controls currently cover ASCII Info, not XMP or full string encodings.

The companion [render manifest](rendering.json) retains the target branch's six
independent graphics/JPX controls (42 observations). Annotation rendering
expansion belongs to its appearance feature review. This services bundle changes
no render tolerance and does not interpret comparative output as an oracle.

## Hosted execution

The [capability workflow](../../.github/workflows/capabilities.yml) runs source
and harness checks on Ubuntu, macOS and Windows. Actual engines run on explicit
GitHub-hosted Ubuntu and macOS runners, using repository dependency pins and
recorded OS versions. ADR 0031 prohibits local builds, tests, engine runs, corpus
scans and benchmarks while performance lanes are active.

```sh
# Source-only manifest validation:
python tools/capability_probes.py check
python tools/capability_render.py check
# Hosted Ubuntu, after installing the declared baseline dependencies:
python tools/capability_probes.py run --engines pdfium,mupdf,pdfjs,poppler,hayro,pagura --output /tmp/linux.json
# Hosted macOS:
python tools/capability_probes.py run --engines quartz --output /tmp/mac.json
# Combine reports from one revision and workflow:
python tools/capability_probes.py combine /tmp/linux.json /tmp/mac.json --output /tmp/combined.json
```

Reports preserve every baseline column, normalized source/fixture/adapter hashes,
actual executable identity, configuration and workflow provenance. Joining rejects
mixed revisions, fixture bytes, workflow runs and overlapping or missing subjects.
Unavailable adapters and missing measurements remain open; they never become N/A.
`pass` requires the independent expected value or a valid invalid-input rejection.
Wrong results, rejected valid inputs, named unsupported features, resource limits,
timeouts, process crashes and unavailable engines remain distinct observations.
A limit never passes an invalid-input rejection control. Native diagnostics retain
byte counts and hashes, not source-controlled log contents.

Pagura must pass every selected expectation. Competitor discrepancies are retained
for adjudication without redefining the expectation. Measurement completion is
separate from correctness: all implemented paths must yield a classified result,
and missing adapters remain visible. A passing workflow cannot establish parity.

Adapter controls cover pdf.js Map/lazy-payload semantics, public MuPDF tree walking,
ASCII filename handling, exact source scope, duplicate page occurrences, payload
bounds, stale output, invalid schemas, process failure and report provenance.
The service fixtures and decoder/viewer tests additionally cover AF/RF selection,
immutable attachment authentication, collections, exact metadata packet identity,
source image bytes and stored thumbnails. Those product controls are separate from
the still-open cross-engine comparisons for the expanded APIs.
