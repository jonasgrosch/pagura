# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
catalog and first-page annotation attachment bytes, and text** using independently authored populated/empty
controls and a non-PDF rejection control. Pagura joins all six baseline columns,
yielding 126 document-service cells. Its observations use the public CLI only.
G01 remains open: these eighteen cases do not establish family or product parity.
The companion [render manifest](rendering.json) adds six independent pixel
oracles: blank and rectangle controls, fractional geometry, JPX grayscale and straight/
premultiplied JPX alpha. Its 42 cells use the public Pagura CLI and all six
existing public render adapters. The ledger retains six competitor columns;
Pagura is an additional measured subject, never a replacement baseline.

| Engine | Public adapter paths | Explicit adapter gaps | Hosted execution |
|---|---|---|---|
| Pagura (subject) | CLI inspect, metadata, attachments, extract-attachment and text | Object-local metadata/AF/RF and broader operation configurations remain open | Ubuntu, pinned Rust, default features |
| PDFium | Pages, metadata, catalog/page attachments, text | Optional V8/XFA builds not covered | Ubuntu, locked pypdfium2 wheel |
| MuPDF | Pages, metadata, catalog/page attachments, text | Optional providers not covered | Ubuntu, locked PyMuPDF wheel |
| pdf.js | Pages, metadata, catalog/page attachments, text | Browser configuration not covered | Ubuntu, locked Node legacy build; eval disabled |
| Poppler | Pages, metadata, catalog/page attachments, text | Other bindings/configurations not covered | Ubuntu, OS GLib/GI packages |
| hayro | Pages, ASCII Info metadata | Attachment and text reconstruction not implemented in this adapter | Ubuntu, locked standalone Rust workspace |
| CoreGraphics | Pages, Info metadata, catalog/page attachments via public object/stream APIs | Text reconstruction not implemented in this adapter | macOS system framework; no PDFKit |

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
The annotation increment also uses the [PDFium annotation API](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/public/fpdf_annot.h),
[PyMuPDF annotation API](https://pymupdf.readthedocs.io/en/latest/annot.html),
[Poppler annotation API](https://poppler.freedesktop.org/api/glib/poppler-Poppler-Annotation.html)
and [CoreGraphics page dictionary](https://developer.apple.com/documentation/coregraphics/cgpdfpage/dictionary).
Published bindings and pinned pdf.js 6.4 descriptor fields were inspected to confirm
public API shapes. The adapter logic and fixtures were authored independently; no
competitor implementation was copied or translated.

## Execution and evidence

The hosted [capability workflow](../../.github/workflows/capabilities.yml) runs
source checks and harness controls on Ubuntu, macOS and Windows, then actual
engine probes on Ubuntu and macOS. Competitor packages/builds stay outside the
product checkout. No product dependencies change. While ADR 0031 reserves the
local machine, run engines/tests only in hosted CI.

```sh
# Source-only validation, without imports or invocation of external engines:
python tools/capability_probes.py check
python tools/capability_render.py check
# Hosted Ubuntu, after installing the OS Poppler GLib/GI packages:
python tools/competitors.py setup --engines pdfium,mupdf,pdfjs,hayro
cargo build --release --locked --offline --bin pagura
python tools/capability_probes.py run --engines pdfium,mupdf,pdfjs,poppler,hayro,pagura --output /tmp/linux.json
python tools/capability_render.py run --engines pdfium,mupdf,pdfjs,poppler,hayro,pagura --output /tmp/render-linux.json
# Hosted macOS:
python tools/competitors.py setup --engines quartz
python tools/capability_probes.py run --engines quartz --output /tmp/mac.json
python tools/capability_render.py run --engines quartz --output /tmp/render-mac.json
# Join reports from one workflow run/revision (no engine execution):
python tools/capability_probes.py combine /tmp/linux.json /tmp/mac.json --output /tmp/combined.json
python tools/capability_render.py combine /tmp/render-linux.json /tmp/render-mac.json --output /tmp/render-combined.json
```

Each platform publishes `capability-document-services-<os>`, even when a probe
fails. After successful platform jobs, `capability-document-services` combines
all 70 cells, requiring disjoint engine selections and identical revision,
workflow-run, source and fixture hashes. Reports record environment, adapter,
manifest, lock and native binary identities. Python/pdf.js/hayro dependencies
are pinned; Poppler and CoreGraphics use the actual recorded OS versions, not
an asserted lock pin. Workspace/cache paths become placeholders. Native
diagnostics retain byte counts and hashes rather than raw logs.

`capability-rendering-<os>` and `capability-rendering` retain the pixel observations
separately. Joining requires all six baselines plus Pagura. The CLI version and
actual Pagura binary hash are recorded; the hosted workflow builds default
features from its checkout using the pinned toolchain. Outside that workflow,
a pre-existing binary has no asserted source revision. The exact PDF/oracle bytes and normalized RGB identities are hashed.
P6/PAM/RGBA adapter output is normalized to RGB over white without resizing,
alignment or inferred tolerance. Every pixel must meet the fixture's existing
per-channel bound at the recorded dimensions. There is no majority-vote oracle.
Fresh output, matching dimensions and attested comparisons are required for a
pass; a successful process exit or leftover image cannot pass. Nonzero renderer
exits remain unclassified adapter failures until investigated, never presumed
backend rejections or unsupported features. These probes do not measure or
compare the render adapters' timing output. Embedded ICC/sYCC and Indexed JPX
alpha from the separate R02 implementation PR will be added after consolidation.
The `--images` option retains actual/oracle PPM pairs for these project-owned
synthetic fixtures; hosted `capability-render-images-<os>` artifacts allow visual
adjudication without committing measured output. A pixel disagreement can reflect
colour, sampling or antialiasing policy and is not by itself an established engine
defect. No tolerance is relaxed to turn an observation into a pass.

`pass` requires the independent expectation, or an engine rejection for the
invalid-input control. A wrong result is `incorrect`; a rejection of valid input
is `rejected`. `unsupported` requires a named explicit backend refusal; `limit` records a bounded
resource refusal and never passes an invalid-input control; the
adapters do not invent one from exceptions. Timeout, adapter errors, absent
adapter paths and unavailable engine installations remain distinct outcomes.
CI requires a completed observation on each implemented adapter path: `pass`,
`incorrect`, a valid-input rejection or a named explicit unsupported result.
Resource limits remain distinct and fail the measurement gate until scope/budget
adjudication; an adapter output bound cannot establish engine capability.
Competitor differences are evidence, so they stay visible without failing the
measuring tool. Broken/unavailable adapters, engine installations and timeouts
fail the workflow. The seven explicitly unimplemented case/engine cells stay
visible too. CI success therefore cannot establish engine correctness or release
readiness. The added Pagura column must pass every selected independent oracle
for its job to succeed; a measured Pagura disagreement remains in the report
and fails this check.

Harness controls check wrong values, boolean/numeric confusion, rejection
evidence, invalid envelopes, missing/duplicate cells, unattested passes, malformed
output, timeouts and missing executables. Report joining also rejects mixed
revisions, fixtures, source hashes or workflow runs and incomplete/overlapping
engine coverage. These controls use stubs, not competitor libraries.
Pixel controls additionally cover wrong/corrupt output, stale files, dimension
and hash mismatches, tolerance drift and process crashes.

## Pagura document-service observations

The [subject adapter](../adapters/capability_pagura.py) reads the public CLI's
versioned reports. Page dimensions use the normalized crop box, rotation and
UserUnit. Info probes compare Title/Author; null values normalize to empty strings
for the existing cross-engine contract. Attachments use the Unicode name when
present and ASCII F bytes otherwise; broader legacy encodings remain unprobed.
EF/UF is selected when present, otherwise EF/F, and the extracted bytes are hashed.
The destination is an adapter-owned fixed temporary path; PDF filenames never
select filesystem paths. Text probes read page zero and apply the same CRLF/LF
normalization as the baseline adapters.

The actual subject binary is versioned and hashed using the shared render/service
identity path. Reports retain all six baselines and the subject; joining refuses
a report set that omits any of them. A Pagura disagreement fails CI. The subject
adapter recognizes an input refusal only from exit 1 and a known engine error
prefix; crashes, usage/IO failures and unknown messages remain adapter errors.
Native diagnostics are represented by hashes. Commands share a 20-second operation
deadline, with 64 KiB report/diagnostic and 64 MiB extracted-payload bounds. These
adapter bounds are separate from product limits. Offline stub controls cover
stale output, path selection, invalid schemas, boolean/numeric confusion, failures,
timeouts and unavailable binaries. Current-head hosted execution is pending.

## Attachment API controls

The original and companion attachment cases exercise EF/F-only and EF/F+UF
stream dictionaries with F/UF filenames (ISO 32000-2 7.7.4 Table 32 and 7.11.3
Table 43). Both retain the same independently authored payload expectation.
The pinned pdf.js 6.4 API returns a Map of descriptors and retrieves uncached
bytes through `getAttachmentContent(id)`. The adapter checks those shapes;
offline Node controls cover lazy bytes, already available bytes, empty Maps,
null results and unexpected return types. Treating the Map as an object would
incorrectly discard attachments; the controls prevent that adapter regression.
The initial expanded reports predate this fix and do not establish a pdf.js
defect. Only reports with the corrected adapter/source hashes are usable for
this comparison. Raw reports remain in CI artifacts, outside Git.

## First-page annotation attachment controls

`annotation-attachments` enumerates only FileAttachment annotations on page zero,
returns sorted filename/SHA-256 records, and preserves duplicate occurrences. It
is separate from catalog `attachments`. Eight additional cases cover page-only
and shared catalog/page specifications, a link before duplicate file annotations,
a file appearing only on page two, catalog-only and empty pages, invalid input,
and catalog enumeration of a page-only file. Each positive case uses the same
authored payload with complete EF/F and EF/UF entries (Table 43); annotation
placement follows 12.5.6.15 Table 187. Names are ASCII in this probe slice.

Pagura uses `attachments --page 0` and extracts each original Annots index through
`extract-attachment --page 0`. PDFium borrows public annotation/attachment handles
while the page is open; PyMuPDF filters the first page's annotation iterator;
pdf.js uses `getAnnotations({intent: "any"})` then `getAttachmentContent(fileId)`;
Poppler uses annotation mappings and file attachment objects; CoreGraphics walks
the first page dictionary and decodes the selected EF stream. No adapter activates
an attachment or treats its filename as an output path. Hayro remains an explicit
adapter gap for both catalog and annotation attachment operations.

Offline contract tests cover index/scope preservation, duplicate results, handle
cleanup on failure, lazy descriptor/byte retrieval, wrong API shapes and adapter
bounds. New annotation enumeration is capped at 1,024 entries (128 for CoreGraphics),
with decoded payload checks of 64 MiB (64 KiB for CoreGraphics). These post-decode
checks do not promise bounded allocation inside competitor APIs; the harness has
a separate process deadline. Resource refusals never establish feature support.
[Hosted run 40276014297](https://github.com/jonasgrosch/pagura/actions/runs/40276014297)
at merge revision `29236241a454a6749340638a14095a363128ccad` records 111 passes
and 15 explicit adapter gaps across 126 cells: Pagura, PDFium, MuPDF, pdf.js and
Poppler each pass all 18 cases, CoreGraphics 16 and hayro five. The missing paths
are thirteen hayro attachment/text cells and two CoreGraphics text cells. There
are no incorrect observations in this run. Later renderer changes require new
current-head evidence; broader attachment encodings,
password/configuration combinations and performance remain separate work.

## Initial rendering observations

[Hosted run 40256476588](https://github.com/jonasgrosch/pagura/actions/runs/40256476588)
measured the six controls against all six baselines at merge revision
`dfccec8472a614ef49a2e8a0264e37cdfb7c0a69`: 20 pixel-oracle passes and 16
incorrect observations. Both empty/integer-rectangle controls passed everywhere.
pdf.js met all six bounds; the other disagreements involve fractional coverage,
grayscale resampling or embedded alpha. This run predates the Pagura column.
The JPX grayscale control was mislabeled `jpx.rgb` in those reports; its fixture
and oracle bytes have not changed.

The actual normalized image hashes repeat across the two hosted runs
40256137477 and 40256476588. Sample inspection is consistent with resampling differences in
the grayscale/straight-alpha images, opaque partial-alpha samples in the hayro/PDFium
premultiplied-alpha output, and non-neutral colour in Poppler's red-alpha input.
These observations still need matched uncompressed controls and explicit
sampling/configuration adjudication; they do not establish competitor defects.
No oracle or tolerance was changed after measurement. Reproduce from the
reports' exact source, native-artifact and fixture hashes, not the current branch.

## Next G01 increments

- Extend Pagura observations beyond the initial rendering and document-service controls,
  including explicit missing typed metadata/attachment APIs, before claiming
  comparative parity.
- Resolve the remaining hayro/CoreGraphics adapter gaps against their public API
  contracts; do not implement a substitute engine and attribute its capabilities
  to the baseline.
- Probe tag subvariants, optional XFA/V8 builds, forms/events, annotation/layer
  state, save/reopen, redaction and signatures, each with independent controls.
- Add explicit resource-limit controls and browser/configuration coverage, and
  bind current results to each scoped capability subvariant. A successful basic
  fixture never closes an entire [capability family](../../docs/CAPABILITIES.md)
  or a release gate.
