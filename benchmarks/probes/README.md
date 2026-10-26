# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
embedded-file bytes and text** using independently authored populated/empty
controls and a non-PDF rejection control. All six baseline columns are retained.
G01 remains open: these nine cases do not establish family or product parity.

| Baseline | Public adapter paths | Explicit adapter gaps | Hosted execution |
|---|---|---|---|
| PDFium | Pages, metadata, attachments, text | Optional V8/XFA builds not covered | Ubuntu, locked pypdfium2 wheel |
| MuPDF | Pages, metadata, attachments, text | Optional providers not covered | Ubuntu, locked PyMuPDF wheel |
| pdf.js | Pages, metadata, attachments, text | Browser configuration not covered | Ubuntu, locked Node legacy build; eval disabled |
| Poppler | Pages, metadata, attachments, text | Other bindings/configurations not covered | Ubuntu, OS GLib/GI packages |
| hayro | Pages, ASCII Info metadata | Attachment and text reconstruction not implemented in this adapter | Ubuntu, locked standalone Rust workspace |
| CoreGraphics | Pages, Info metadata, EmbeddedFiles via public object/stream APIs | Text reconstruction not implemented in this adapter | macOS system framework; no PDFKit |

A missing adapter path is `adapter-unavailable`, not a claim that an engine
cannot perform the work. Each platform report also retains the other platform's
columns as `not-run`. The combined report must cover every engine exactly once.
Neither outcome becomes an unsupported feature or a release exemption.

The [manifest](document-services.json) owns exact expectations. The fixture builder
in [capability_probes.py](../../tools/capability_probes.py) writes small, synthetic
classic-xref PDFs without using Pagura or a competitor writer. Expectations are
not generated from measured output or majority agreement. Text normalization
only replaces CRLF with LF and trims leading/trailing LF; whitespace and character
differences otherwise fail. Attachment names and SHA-256 payload hashes must match.
The current metadata fixtures are ASCII; they do not exercise PDFDocEncoding,
UTF-16, XMP or malformed strings. Page fixtures do not yet cover rotation/UserUnit.

The adapters use documented public APIs:
[pypdfium2](https://pypdfium2.readthedocs.io/en/stable/python_api.html),
[PyMuPDF document](https://pymupdf.readthedocs.io/en/latest/document.html) and
[page](https://pymupdf.readthedocs.io/en/latest/page.html),
[pdf.js PDFDocumentProxy](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib-PDFDocumentProxy.html),
[Poppler GLib document](https://poppler.freedesktop.org/api/glib/PopplerDocument.html) and
[attachment](https://poppler.freedesktop.org/api/glib/PopplerAttachment.html),
[hayro 0.8.0 page](https://docs.rs/hayro-syntax/0.8.0/hayro_syntax/page/struct.Page.html) and
[metadata](https://docs.rs/hayro-syntax/0.8.0/hayro_syntax/metadata/struct.Metadata.html),
and [CoreGraphics CGPDFDocument](https://developer.apple.com/documentation/coregraphics/cgpdfdocument).
No competitor implementation source was consulted for these adapters.

## Execution and evidence

The hosted [capability workflow](../../.github/workflows/capabilities.yml) runs
source checks and harness controls on Ubuntu, macOS and Windows, then actual
engine probes on Ubuntu and macOS. Competitor packages/builds stay outside the
product checkout. No product dependencies change. While ADR 0031 reserves the
local machine, run engines/tests only in hosted CI.

```sh
# Source-only validation, without imports or invocation of external engines:
python tools/capability_probes.py check
# Hosted Ubuntu, after installing the OS Poppler GLib/GI packages:
python tools/competitors.py setup --engines pdfium,mupdf,pdfjs,hayro
python tools/capability_probes.py run --engines pdfium,mupdf,pdfjs,poppler,hayro --output /tmp/linux.json
# Hosted macOS:
python tools/competitors.py setup --engines quartz
python tools/capability_probes.py run --engines quartz --output /tmp/mac.json
# Join reports from one workflow run/revision (no engine execution):
python tools/capability_probes.py combine /tmp/linux.json /tmp/mac.json --output /tmp/combined.json
```

Each platform publishes `capability-document-services-<os>`, even when a probe
fails. After successful platform jobs, `capability-document-services` combines
all 54 cells, requiring disjoint engine selections and identical revision,
workflow-run, source and fixture hashes. Reports record environment, adapter,
manifest, lock and native binary identities. Python/pdf.js/hayro dependencies
are pinned; Poppler and CoreGraphics use the actual recorded OS versions, not
an asserted lock pin. Workspace/cache paths become placeholders. Native
diagnostics retain byte counts and hashes rather than raw logs.

`pass` requires the independent expectation, or an engine rejection for the
invalid-input control. A wrong result is `incorrect`; a rejection of valid input
is `rejected`. `unsupported` requires a named explicit backend refusal; the
adapters do not invent one from exceptions. Timeout, adapter errors, absent
adapter paths and unavailable engine installations remain distinct outcomes.
The implemented adapter paths must pass for CI to succeed. The six explicitly
unimplemented case/engine cells remain visible; they cannot establish release
readiness. An unexpected API absence on an implemented path fails the job.

Harness controls check wrong values, boolean/numeric confusion, rejection
evidence, invalid envelopes, missing/duplicate cells, unattested passes, malformed
output, timeouts and missing executables. Report joining also rejects mixed
revisions, fixtures, source hashes or workflow runs and incomplete/overlapping
engine coverage. These controls use stubs, not competitor libraries.

## Next G01 increments

- Add Pagura observations through its actual public surfaces, including explicit
  missing typed metadata/attachment APIs, before claiming comparative parity.
- Resolve the remaining hayro/CoreGraphics adapter gaps against their public API
  contracts; do not implement a substitute engine and attribute its capabilities
  to the baseline.
- Probe tag subvariants, optional XFA/V8 builds, forms/events, annotation/layer
  state, save/reopen, redaction and signatures, each with independent controls.
- Add explicit resource-limit controls and browser/configuration coverage, and
  bind current results to each scoped capability subvariant. A successful basic
  fixture never closes an entire [capability family](../../docs/CAPABILITIES.md)
  or a release gate.
