# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
embedded-file bytes and text** using independently authored populated/empty
controls and a non-PDF rejection control. All six baseline columns are retained.
G01 remains open: these ten cases do not establish family or product parity.
The companion [render manifest](rendering.json) adds six independent pixel
oracles: blank and rectangle controls, fractional geometry, JPX grayscale and straight/
premultiplied JPX alpha. Its 42 cells use the public Pagura CLI and all six
existing public render adapters. The ledger retains six competitor columns;
Pagura is an additional measured subject, never a replacement baseline.

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
python tools/capability_render.py check
# Hosted Ubuntu, after installing the OS Poppler GLib/GI packages:
python tools/competitors.py setup --engines pdfium,mupdf,pdfjs,hayro
python tools/capability_probes.py run --engines pdfium,mupdf,pdfjs,poppler,hayro --output /tmp/linux.json
cargo build --release --locked --offline --bin pagura
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
all 60 cells, requiring disjoint engine selections and identical revision,
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
is `rejected`. `unsupported` requires a named explicit backend refusal; the
adapters do not invent one from exceptions. Timeout, adapter errors, absent
adapter paths and unavailable engine installations remain distinct outcomes.
CI requires a completed observation on each implemented adapter path: `pass`,
`incorrect`, a valid-input rejection or a named explicit unsupported result.
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

- Extend Pagura observations beyond rendering through its actual public surfaces,
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

## R02 image integration controls

The image bundle adds 214 controls to the six existing graphics/image
controls: unsigned 1–16-bit and mixed-precision sYCC, embedded ICC, native-size
sampling, matching ordinary RGB/soft-mask images, ranked profile choices,
physical Lab input, palette-backed PDF Indexed opacity, masks/groups/interpolation
and resource-aware inline framing. Signed restricted Gray/RGB TRC controls add full-precision negative clipping,
matched unsigned and RGB pairs, mixed signed palette channels and profile
fallback. The 220-case inventory therefore has 1,540
observations across Pagura and six baselines. These are scoped controls, not
family support verdicts; full functional native/WASI checks and comparative
results for this source revision are pending hosted execution.

The existing PR182 precision fixtures and PR187 matched controls retain their
original pixels and tolerances. New integration expectations use the primary
formulae recorded in the [image painting requirements](../../references/requirements/paint.json)
and [JPEG 2000 requirements](../../references/requirements/jpx.json).
Profile-selection controls distinguish malformed metadata from unavailable
profiles/transforms. Future signatures test fallback policy rather than claiming
that an unknown extension is valid current ICC conformance. The repository
contains no third-party implementation or licensed standard text.

The consolidated image source starts directly from `render/one-pass`
`73e1ab018d9c23f24c374de1f3a4441b119a5365`. Reviewed PR182 source
`e16f5227f1290b51a7447ffd273f7dbf27e9c001` supplies the original unsigned
sYCC precision controls; PR187 source `80e9aed8f8c7cbb1b23da7b536649582bc0607af`
supplies their matched ordinary RGB/soft-mask controls. Original refs are preserved.

The signed restricted-TRC diagnostic at
`cb444312450e900a5b23b7b344286ee510d63cd6`, [run 40341090579](https://github.com/jonasgrosch/pagura/actions/runs/40341090579),
retained the pre-fix implementation: Pagura passed 212 controls and returned
Unsupported on the four `signed-trc-{gray,rgb}-{8,16}` cases. The workflow failed
for those expected subject errors; it is diagnostic evidence, not acceptance.
Linux render-report artifact `12660430830` has SHA-256
`74d52c3d16f0c01406c8708db9c73c18befa5762e5b171a246bc29386af8a743`; macOS
artifact `12660000540` has SHA-256
`213de85810c8ed9aa9befe7d7dccf415291d8aacf590fbdcb065f87c29ecefaa`.

| Baseline configuration | Signed-control observation | Control limitation |
| --- | --- | --- |
| PDF.js 6.4.299 | Each signed result is byte-identical to its clipped unsigned JPX pair. | Both differ from the ICC-managed oracle; ordinary RGB controls pass. |
| MuPDF 1.28.5; Quartz on macOS 26.6.2 | Signed results differ from clipped unsigned pairs. | Unsigned ICC and ordinary RGB controls pass. |
| PDFium 156.0.8076.0; hayro 0.8.0 | Eight-bit signed gray output retains the storage-offset sequence. | Both signed and unsigned ICC results differ from their oracles. |
| Poppler 24.02.0 | Signed observations remain unadjudicated. | Ordinary RGB controls also fail. |

For PDF.js, eight-bit signed and unsigned gray controls both produce
`[0, 0, 0, 0, 64, 127]`; the primary ICC oracle and ordinary RGB control produce
`[0, 0, 0, 0, 137, 187]`. This isolates sample clipping from colour interpretation
without making an ICC family-parity claim. Measured output never supplies an oracle.

Source commit `8a27bb8a3` implements the T.800 Table I.9 clipping policy only for
selected XYZ Gray/RGB matrix/TRC input/display profiles. General signed Lab/LUT
profiles remain Unsupported unless a later restricted candidate applies.
Mixed signed/unsigned palette channels, opacity modes and candidate fallback
have independent controls and public raw-sample tests. The reference renderer
separately unassociates stored samples before Decode/range mapping; the fixed
`lab-alpha-2` and `premultiplied-inverted-decode` oracles expose ordering mistakes.
No tolerance was relaxed. Precision above sixteen bits, broader Part 2 models
and layering remain R03. Native/WASI/browser and current implementation acceptance
remain tied to hosted results for the published source revision (ADR 0031).
