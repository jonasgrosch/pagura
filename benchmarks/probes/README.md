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
fallback. This 220-case R02 subset therefore has 1,540
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
No tolerance was relaxed. Precision 17–38 and permitted JPX baseline models
and first-image metadata remain R03. PDF 7.4.9 prohibits multilayer composition
and animation; it does not exclude every Part 2 tool. Native/WASI/browser and current implementation acceptance
remain tied to hosted results for the published source revision (ADR 0031).

## R03 lossless JPEG compatibility controls

The image inventory adds twelve Huffman lossless JPEG subjects and twelve
matching ordinary RGB controls, for 244 render cases and 1,708 observations.
They cover Gray/RGB, predictors, whole-row restarts, separate scans, cropped and
fractional component sampling, point transforms, colour conversion and native
precision. The 66 codec vectors include every precision from two to sixteen
bits, all seven predictors, four Huffman tables, CMYK/YCCK, malformed fields and
explicit arithmetic/hierarchical boundaries. Three literal bitstreams separately
pin category 16, negative half-difference rounding and overflowing predictions.
Public ImageSource and exact memory/work limits are checked in the image tests;
the existing host suite exercises the render subjects through native, WASI,
session-client and partial-tile paths. This checkpoint passed at
`c50727d340db1236c98eb769e002771d680e790a`: full Ubuntu/macOS/Windows
[functional run 40357238745](https://github.com/jonasgrosch/pagura/actions/runs/40357238745),
[editing run 40357236212](https://github.com/jonasgrosch/pagura/actions/runs/40357236212)
and [render run 40357236500](https://github.com/jonasgrosch/pagura/actions/runs/40357236500).
All 244 Pagura render controls pass in the 1,708-cell report, measured at merge
`198c2b95d4af46e2ab4adaf021741b4f708d798c`. Artifact `12666298024` has SHA-256
`bdb55a05509897da10e51347c806e8a949226cf9f1a7726d1c2b3767ccb7ba7a`.
These results remain separate from acceptance of later source changes.

Lossless JPEG is a compatibility extension beyond PDF 7.4.8's required
baseline/progressive byte processes. The [image requirements](../../references/requirements/image.json)
record T.81 H's native sample rules and the explicit full-range byte
normalization policy. Neither a measured renderer nor the product supplies
expected pixels. The implementation reuses the frame parser, Huffman reader,
scan loop and colour output; arithmetic lossless and hierarchical processes
remain Unsupported.

The pre-implementation diagnostic at `ac2c7d5b9d01be4556b20c2e2f2dcea086b8231d`,
[run 40347171720](https://github.com/jonasgrosch/pagura/actions/runs/40347171720),
contains ten lossless subjects and ten matched RGB controls. Pagura passed its
230 controls and refused all ten subjects, so this failed run is diagnostic
evidence. Linux render-report artifact `12663225028` has SHA-256
`0833a0cc4a35ca06f103b83051df388471d51b0d1f04fc53986aae884fb46340`;
macOS artifact `12661079405` has SHA-256
`d50b1fa9b5e8b80b1f891dfbf0522df4d89c055062324e380734e0c2b4b996a4`.

| Pinned baseline | Subject observations | Paired controls |
| --- | --- | --- |
| PDFium 156.0.8076.0 | All three 8-bit Huffman subjects pass. Two 16-bit Huffman and five arithmetic subjects differ from their oracles. | All ten pass. |
| MuPDF 1.28.5, PDF.js 6.4.299, hayro 0.8.0, Quartz on macOS 26.6.2 | All ten subjects differ from their oracles. | All ten pass. |
| Poppler 24.02.0 | Subjects remain unadjudicated. | All ten differ from their oracles. |

The exact diagnostic inputs and reports remain bound to that preserved source
revision. Arithmetic render probes are not implementation acceptance cases;
the current codec vectors retain their explicit Unsupported boundary. The
original Huffman expected pixels and tolerance zero remain unchanged. These
observations justify the 8-bit compatibility work; they do not establish a
general lossless, arithmetic or high-precision baseline-support verdict.

## R03 bitonal extensions

The accepted bitonal checkpoint adds eight subjects with eight ordinary RGB controls: CCITT
uncompressed continuation and resumed run coding, plus JBIG2 extended adaptive
templates, typical prediction and unknown-length framing. That checkpoint contains
260 render cases and 1,820 observation cells. All new pixels come from
literal bit patterns or original geometric bitmaps at native pixel size; paired
controls have identical PPM oracles and zero tolerance. Its acceptance is bound to
`6e87a6030ae206322aa813388b41e18363d5af81`: all twelve retained PR gates pass,
including three-OS [functional run 40364384706](https://github.com/jonasgrosch/pagura/actions/runs/40364384706),
[editing run 40364384681](https://github.com/jonasgrosch/pagura/actions/runs/40364384681)
and [capability run 40364384650](https://github.com/jonasgrosch/pagura/actions/runs/40364384650).
The combined report has 260/260 Pagura passes, measured at merge
`c231a3e06676d6ab97837a553923fe2bc372c184`. Artifact 12670344135 ZIP SHA-256 is
`02bcad88b4fd2856d979287ed890f904af9dbd109819d7610f187c471cd2a386`; report
SHA-256 is `a1dcb4632eb15780ae337d7c514f76f08a75f7f9985ddd68a4f8bb76a450f8d3`.
PDFium, MuPDF, PDF.js, hayro and Quartz differ on all eight new subjects while
their eight RGB pairs pass. Poppler controls fail, leaving its subjects
unadjudicated. These scoped negative observations do not establish whole-family
absence, and these green results do not certify later source changes.

The 65 fax vectors cover every T.4 Table 5 / T.6 Table 4 literal and exit, both
run colours, 1-D/mixed/2-D coding, words spanning several T.6 rows, byte alignment,
polarity, row-count/EOFB interaction and malformed framing. The exact continuation
test budgets 45 bit/lookahead operations and `6*sizeof(usize)+21` live bytes for
twenty width-one rows, with one-short refusals. T.88's prohibition of the same
extension inside JBIG2 MMR remains in its separate existing decoder.

The 66 JBIG2 vectors exercise all twelve adaptive pixels separately and together,
overlap, coordinate limits, typical rows, every truncated coordinate header,
noncausal pixels and unknown-length marker/count boundaries. The source is the
official [T.88:2018 package](https://www.itu.int/rec/T-REC-T.88-201808-I/en),
identical to the ISO 14492:2019 edition referenced by PDF 7.4.7. Its Figure 50 and
explicit byte 0–23 list conflict with a sentence calling the field 32 bytes.
The [requirements](../../references/requirements/jbig2.json) record the source
hashes and the decision to follow the explicit twelve-pair layout; no unspecified
bytes or alternate layout are guessed. Unknown-height allocation uses the actual
row count, and the original exact budgets are extended to the 65,536 contexts.


## R03 JPEG/JPX marker and baseline-container controls

The fourteen image inputs from diagnostic `9e97615f228fb3f5cb0ea76f14ddf20872659a51`,
[run 40368464986](https://github.com/jonasgrosch/pagura/actions/runs/40368464986),
are preserved byte-for-byte under `image/` with identical PPM oracles and tolerance
zero. The integrated [hosted rendering report](https://github.com/jonasgrosch/pagura/actions/runs/40376025925)
at `c04da5c605d84c7e66c7de3582f28abc61738a97` passes all 274 Pagura controls
across 1,918 cells. Artifact `12675450769` has ZIP SHA-256
`14083a8db347da3e3b446e7802134f707b81a05fca7f039e1168c160b43a98d6`
and report SHA-256
`f7c0f1d681a4482931c2e91336ecfc21b730f7a0a372b9b495feedc9cd7619e4`.
This rendering result is separate from full product acceptance. The original
missing-DNL test now asserts Syntax: T.81 B.2.5 requires DNL for SOF Y=0, so its
absence is malformed rather than an unimplemented process.
The corrected checkpoint `20cf20993f229ce589366ceacf9f14446293df00` passes all
three OS jobs in [functional run 40378326876](https://github.com/jonasgrosch/pagura/actions/runs/40378326876),
[editing run 40378333349](https://github.com/jonasgrosch/pagura/actions/runs/40378333349),
and all 274 Pagura controls in [capability run 40378339724](https://github.com/jonasgrosch/pagura/actions/runs/40378339724).
Its artifact `12678465710` has ZIP SHA-256
`0752e6672392e2afcc63fa092d8465f4e908a8faf1487d312c37d50eecdb35dd`
and report SHA-256
`55ea413098f3a1a54c26502ab9c74dceb255c750eda5b1f2f5010f95e9942a1e`.
Performance base `9f9801a754203009a314644ba22afbfdc4f3e58b` is integrated at
`d167185d2b3f7b950a50fce7d71b8c6943bb0707`. Its three operating-system jobs in
[functional run 40384519486](https://github.com/jonasgrosch/pagura/actions/runs/40384519486),
[editing run 40384519675](https://github.com/jonasgrosch/pagura/actions/runs/40384519675)
and all seven [capability jobs 40384519618](https://github.com/jonasgrosch/pagura/actions/runs/40384519618)
pass. The combined report records all 274 Pagura controls and 1,918 cells at
PR merge revision `600a6fad75d8e3a59995d9e673c6cc6d2ccd8169`.
Artifact `12681226063` has ZIP SHA-256
`ee743b8d40d67f55c5a66fd0c88aae78b19224e6d0003a34ec0be5c3ea7c4406`
and report SHA-256
`ab6d9d827a492df2386010d47b1dedd5c85a1d79d4023c94164bb83a6e651a94`.
The original diagnostic inventory had those fourteen image cases plus
blank/rectangle controls, for 112 cells.
Its eight Pagura subject failures remain `adapter-error`, as recorded by the
then-current harness; source review separately identified the relevant refusal
and framing paths. Historical outcomes are not rewritten.

| Subject | Pinned baselines matching the exact RGB oracle and applicable control |
| --- | --- |
| DNL with deferred Y=0; progressive DNL | MuPDF 1.28.5 |
| DNL with unchanged Y=8 | PDFium 156.0.8076.0, MuPDF 1.28.5, PDF.js 6.4.299, Quartz macOS 26.6.2 |
| DNL redefining Y=16 to 8 | PDFium, MuPDF, Quartz |
| PLM alone; PLM plus zero CRG | PDFium, MuPDF, PDF.js, Quartz |
| Zero CRG alone | PDFium, MuPDF, PDF.js, hayro 0.8.0, Quartz |
| jpx/jpxb-only with corrected rreq | PDFium, MuPDF, PDF.js, hayro |

MuPDF matches the literal oracle with zero maximum channel error on all fourteen
image subjects/controls. Poppler 24.02.0 fails the image controls, so its image
subjects remain unadjudicated. Quartz fails jpxb-only while the otherwise
identical jp2-compatible control passes. The Linux report artifact 12670429976
has report SHA-256
`47625b91b84a3c5078ccd470f8d7f23b400f47549431390b272c8dd5f43c3dcc`;
macOS artifact 12670534766 has report SHA-256
`86836e090fe43fdc59ce3c12180578cb5175e884505f9aa62093fd6e030a73ca`.
The unchanged engine identities and per-case outcomes remain in those reports.

T.81 B.2.5/Table B.10 and E.2.4 permit DNL to replace a nonzero SOF height at the
end of the first complete scan. Shared entropy framing discovers that height
before any sample allocation, then the existing decoder validates the MCU rows.
This adds one first-scan framing traversal to ordinary JPEGs; there is no second
pixel decode or preflight entropy copy. Every inspected byte is charged. The
literal lossless control's work rises from 22 to 60: 4 SOS/length bytes plus 34
entropy-through-EOI bytes, followed by its unchanged 20 sample units and 2 end-marker
inspections. The DNL constant control requires 81 operations and 64 pixels.
After the performance-base merge its byte admission is 128 plane/output bytes
plus `8*(6*sizeof(i16)+sizeof((usize,u8,u8)))` for the DCT conversion-row
allowance: 352 bytes on a 64-bit host, with a one-short refusal. This is a
conservative admission bound, not the live row allocation of a gray image.
Sequential DCT frames no longer reserve nonexistent coefficient planes; the
lossless scalar word path retains its exact 100/99-byte control without a row
allowance. The pixel oracles and 81/80 and 60/59 work boundaries are unchanged.

The bounded [Ubuntu measurement](https://github.com/jonasgrosch/pagura/actions/runs/40375650525)
compared subject `c04da5c605d84c7e66c7de3582f28abc61738a97` with baseline
`6e87a6030ae206322aa813388b41e18363d5af81`, using private workflow revision
`29a7d45c3b6054f484a8eb489ee7070684f889f5`. Both heads used the unchanged
`image/jpeg-budget-huffman` and `image/jpeg-budget-arithmetic` bytes, literal
samples and tolerance 1 in native-size 128×64 gray PDF wrappers. All ten paired
process samples per input passed and produced identical pixels, with three
warmups and no warmup failures. The existing `render-cold` microbenchmark used
2,000 fresh-context renders per block, ten alternating paired blocks and three
warmups per input; raw output retains all 52 block invocations.

| Ordinary JPEG control | Baseline median µs/render | Subject median µs/render | Median paired subject/baseline |
| --- | ---: | ---: | ---: |
| Huffman | 86.53 | 89.33 | 1.03155 |
| Arithmetic | 166.69 | 167.95 | 1.00933 |

This is scoped overhead evidence from GitHub-hosted Ubuntu 24 x64 image
`20261023.334.3`, Rust 1.99.1, without isolated-runner power or idle attestation;
it is not an ADR 0002 release gate or a whole-family performance result.
Artifact `12675381132` (`jpeg-framing-c04da5c60`) retains identities, binary hashes,
input hashes, process samples and raw cold blocks. Its ZIP SHA-256 is
`736b6585a366d340e5852525b7cb7f9ce938c371d8f2d256e92653fa2160c001`;
`regression.json` is
`2b801c5c853ad249aca395d5243eb9b155add1e66479976d45bab37bc6d966b7`
and `cold.json` is
`9af40ef1226d5fa8d28bbcc64ba2641382a65e8ec7a48ab8f66bea0234501ff3`.

PLM uses the existing indexed-fragment owner and PLT length syntax. T.800 A.7.2
allows an Nplm byte list to span markers, while each packet length must finish
inside one marker. Continuation, reversed physical fragment order, malformed
variable integers and exact byte work limits are independent controls. A.9.1's
zero CRG offsets are an identity; nonzero registration remains explicitly
unsupported pending its own rendering evidence.

The same box parser admits a bounded jpx/jpxb-only subset: MinV 1, one immediately
following rreq, supported Display Contents alternatives, and existing first-image
metadata. Unknown required standard/vendor features, unsupported layout boxes,
and malformed masks are refused. FUAM-only unknown metadata does not itself
prevent display. Legacy jp2-compatible behaviour remains unchanged. The
[JPX requirements](../../references/requirements/jpx.json) cite the official
[T.801:2002 edition](https://www.itu.int/rec/T-REC-T.801-200208-S),
[Cor3](https://www.itu.int/rec/T-REC-T.801-200501-S!Cor3) and
[Cor4](https://www.itu.int/rec/T-REC-T.801-200605-S!Cor4), with exact cached-text
hashes and the dated-edition distinction. No primary text is redistributed.

Remaining applicable work includes 17–38-bit components, nonzero CRG, early packed
headers, ordered in-file fragments and cross-references, non-D50 CIELab
illuminants, e-sRGB 20, ROMM-RGB 21,
e-sYCC 24, and the restricted baseline array/nonlinear transforms. PDF 7.4.9 excludes
CIEJab 19, multilayer composition/animation and used per-colour/multiple opacity;
chroma-key metadata is ignored. These boundaries do not erase the permitted
Part 2 obligations. Undefined CCITT selectors and unknown necessary JBIG2
extension identifiers remain explicit boundaries until a concrete applicable
extension is established. All six engine-family ledger cells remain unprobed.

The shared refusal-classification correction from 703d6c4deeba0afc76932793df966be0f3976f77
uses the existing `explicit_refusal` owner from the appearance workstream.
A named backend/CLI refusal is recorded as such; arbitrary nonzero exits stay
unclassified. Negative controls cover both cases without changing pixel acceptance
or historic diagnostic rows.

## R03 enumerated CIELab diagnostic

The private source checkpoint
[`ee7f1aecf197bf55602f22f2101d4d9dc3fffc23`](https://github.com/jonasgrosch/pagura/tree/ee7f1aecf197bf55602f22f2101d4d9dc3fffc23/tools/fixturegen/jpx_lab_probe.py)
keeps product code identical to `20cf20993f229ce589366ceacf9f14446293df00`.
[Run 40380351282](https://github.com/jonasgrosch/pagura/actions/runs/40380351282)
measured six D50 enumerated-14 subjects: default/explicit-default 8-bit,
default 16-bit, explicit custom ranges at 8/16 bits, and mixed 8/12/16-bit
palette output. Each has an unchanged-codestream sRGB control, an independently
calculated RGB control, and a PDF Lab control. With blank/rectangle controls,
these are 26 cases and 182 observed cells. The six malformed-length and five
sign/illuminant/zero-range vectors were not executed or counted in that total.

The [JPX requirements](../../references/requirements/jpx.json) record T.801
M.11.7.4.1/M-18 default and explicit R/O mappings, Cor4 features 56/57, and the
primary D50 white point. Oracles and tolerances were frozen before execution.
All 36 baseline subject cells are `incorrect`; Pagura reports six explicit
`filter.JPXDecode` refusals and passes all 20 controls. MuPDF passes every paired
control, but its five direct Lab subjects have the same RGB digests as their
codec controls; its palette result is grayscale. This is a scoped negative
result, not a family support verdict. Baseline CIELab remains a normative PDF
7.4.9/T.801 M.9.2.4 obligation; this diagnostic established the input/oracle
pairing for the implementation below.

| Pinned baseline | Subject/control adjudication |
| --- | --- |
| MuPDF 1.28.5, wrapper 2.0 | All 18 paired controls pass; six subjects fail with maximum channel error 160–167. |
| PDFium 156.0.8076.0, pypdfium2 5.14.0 | Direct subjects match their sRGB codec-control digests; PDF Lab controls and the palette codec control fail. |
| PDF.js 6.4.299, Node 26.8.1, canvas 1.0.11 | Subjects match their codec-control digests; PDF Lab controls fail. |
| hayro 0.8.0 | Direct subject pixels differ from both codec controls and RGB oracles; palette codec and four PDF Lab controls fail. |
| Poppler 24.02.0, PyGObject 3.48.2 | RGB/codec controls fail; subjects remain unadjudicated against those pairs. |
| Quartz macOS 26.6.2 build 25G83 | jpxb codec and PDF Lab controls fail; subjects remain unadjudicated against those pairs. |

Pagura binary SHA-256 is
`22b78813c9cece861ba6428a7dcb30a724d3812f5cdb6c980479d8324c50bb2f`.
The Linux render job fails on the expected refusals; macOS completes its
observations. All inventory jobs pass; combined-report jobs are skipped. The
platform reports retain their original outcomes and other-platform `not-run`
rows. These historical failures do not assert acceptance of a later implementation.

| Artifact | ID | SHA-256 of ZIP | SHA-256 of report |
| --- | --- | --- | --- |
| Ubuntu report | 12678907140 | `c7c8eb68ff7c16e5cff26b7689c2a90c3ec3fd298b125f192d718c291ede5ba3` | `21e66b279dabb2cddd86a5bee819bb165a70ab69af993ecad6596ff3afaede55` |
| macOS report | 12677922030 | `8aa365172ca2abf724443c4807adced87122e141eb08dc2eb5fe27cad56c3cc5` | `de5ec5fcd6226887adb32649f2fbd619ac706d48447f89f6054151c3b395c7d3` |
| Ubuntu pixels | 12679126815 | `7f80445c28fdf91028d682eb5fc721c343411e3bb8619e88ab6ad16b014cb25e` | — |
| macOS pixels | 12678112882 | `b4199b530e637c32da343f91fb246add18805e6c7545dcf2a3c0536cd5970764` | — |

## R02/R03 enumerated CIELab integration controls

The existing JP2 box parser preserves enumeration 14's default or explicit
seven-field encoding. Colour selection derives each physical interval from the
final unsigned channel precision after palette mapping and channel ordering:
`low = -O*R/(2^bits-1)`, `high = low+R` (T.801 M-18). It supplies those three
intervals to the existing calibrated Lab conversion and image Decode owners.
The ordinary PDF Lab constructor still fixes L* to `[0,100]`; explicit JPEG
2000 encoding can represent a different L* interval. The existing reader
requirements recognise Cor4 features 56/57, while actual colour selection still
checks channel precision, sign, illuminant and the chosen encoding. D50 is the
implemented illuminant. Other illuminants, signed Lab and zero-range records
remain explicit unavailable candidates. The selector can choose a supported
lower-ranked specification; an exhausted Lab list never becomes implicit
DeviceRGB. The historical duplicate-enumeration files exercise tolerance of
malformed groups, as qualified below, and do not establish valid-file fallback. The public decoded-image fallback follows
the same rule; it does not replace selection of an explicit colour candidate.
Zero-range validity remains unadjudicated rather than receiving an invented
Syntax verdict.

The 35 original diagnostic records and all 70 files are unchanged. The original
26 render expectations and all 274 integrated controls are preserved in the
original 323 rendering rows, now retained in a 332-case manifest. The 49
CIELab-area observations include the
original 24 PDFs plus one-/two-bit defaults, a 1/2/16-bit palette, extended
lightness and straight/premultiplied alpha with RGB controls. Three historical
ranked-fallback files are malformed colour groups; their exact observations
remain visible rather than being counted as valid JPX acceptance.
All use the existing independent CIE arithmetic and geometry generators;
neither Pagura nor another renderer supplies expected pixels. At
`L*=120, a*=-80, b*=80`, the declared extended interval gives RGB8
`(158,255,134)`, whereas ordinary PDF clipping gives `(83,255,71)`.
This literal order-sensitive control follows T.42 Appendix II's Lab/XYZ
conversion and the independently solved Bradford/sRGB matrices. One-/two-bit
default b offsets are respectively `3/4` and `3/2`, not rounded integer codes.

Public image tests retain raw samples and post-palette precision for both
interleaved/native requests. All 49 render cases also enter the existing
native/WASI/browser-shim checks and reference-renderer conformance path. Six
malformed EP lengths return Syntax; typed colour selection checks unsupported
sign/illuminant/range boundaries. The colour-record parser's exact source budget
is one `ColorSpecification` and one allocation-initialisation operation;
one-short byte and zero-operation controls must refuse it before insertion.
The [63e render run](https://github.com/jonasgrosch/pagura/actions/runs/40390162187)
measured 323/323 Pagura observations and 2,261 total cells at merge
`fdd6a4c126815a07b3b34030144da1922a9b5bde`. Artifact `12684685868` has ZIP
SHA-256 `91ad9c65a1c2691bcb669dfc94fb09faa158bee2cec97532bc2c0f129894406d`
and report SHA-256
`65ac18d18af959c70fb4780cd23cbe6718ca0caa71101b7f5c2aef0b2613ce85`.
The six malformed subjects below are included in that count. The functional
run at 63e failed a test-only Clippy lint; df04 corrects that assertion without
changing product behaviour or expected pixels. The valid ranking controls
below have separate hosted evidence.


## JPX colour-group validity qualification

T.801 M.11.7.1 explicitly applies to both the Colour Group box (`cgrp`) and
the JP2 Header default group (`jp2h`): each permits at most one enumerated
record (METH1) and one restricted ICC record (METH2), while multiple Any ICC
(METH3) and vendor (METH4) records are permitted. The methods must describe
related/equivalent colours. Neither Cor3 nor Cor4 alters that clause.

Six historical subjects violate the repeated-method rule. They remain byte for
byte identical, with unchanged oracles, tolerances and historical report rows:

| Fixture area | Historical malformed subjects |
| --- | --- |
| `jpx_lab_probe` | `lab-fallback-illuminant`, `lab-fallback-zero-range`, `lab-fallback-rgb` repeat METH1. |
| `paint_jpx_integration` | `precedence-signed`, `approximation-quality`, `stable-equal-priority` repeat METH2. |

Their fixture manifests now label `jpx_validity` and `evidence_scope`; they are
malformed-input observations, not valid JPX acceptance. The existing
`strict_valid` field describes strict PDF opening, not embedded JPEG 2000
format validity, and is unchanged. Lab selector tests are named and documented
as these historical tolerance observations. A distinct ICC RGB alternative to
unsupported Lab would also need an independently established equivalent colour
transform. That file-level fallback remains open; replacing the method byte
alone would not establish it.

Three new `any-icc-*` ranking subjects use permitted repeated METH3 records and
one low-precedence restricted ICC fallback with the same intended exact colour.
Each has an ordinary JP2 control with identical codestream samples and an
independent RGB control: nine new rendering rows. MinV1/rreq follow Cor4;
FUAM declares Part1 feature5 and Any ICC feature44, while DCM needs only
feature5 because the restricted fallback produces the intended colour.
The independent unit-TRC/Bradford-sRGB profile builder and scalar sRGB arithmetic
supply all bytes and expectations. These controls can expose a wrong ranking
choice without requiring a reader that lacks Any ICC to use it. Public image
metadata and allocation-refusal checks now use this valid first-image container.
No product code, reader-feature grant, acceptance tolerance, or prior expected
pixel changed. Family comparison cells remain unprobed.

At source `1ab267de8d0320afe46b058543f14d882a92cb93`,
[run 40395397775](https://github.com/jonasgrosch/pagura/actions/runs/40395397775)
passed all 332 Pagura observations, including the nine valid ranking rows;
the report contains 2,324 cells at merge
`9957038b21ac87b3fe679ef9867e317aeb8afed4`. The six historical malformed
subjects remain separately qualified. Artifact `12686353712` has ZIP SHA-256
`c0b47dc132ed55ac557525432f427290a8df1d5b4f8ef8d69a1f2676b0bde737`
and report SHA-256
`148b32fcebb218b381d1afaf81d2716a6bff533a9692f87c3ffec2e2227c260e`.
Functional run `40395397708` passed Ubuntu, macOS and Windows; editing run
`40395397724` also passed. This evidence predates the first-image integration.


## R02/R03 first-image JPX layout integration

The private [diagnostic source 563fa4eb](https://github.com/jonasgrosch/pagura/tree/563fa4eb73170dc4209e3c939a67bdccea86ec49/tools/fixturegen/jpx_layout_probe.py)
kept product code identical to df04. [Hosted run 40393543009](https://github.com/jonasgrosch/pagura/actions/runs/40393543009)
measured 50 cases and 350 cells: twelve first-image subjects, each with an
unchanged-codestream ordinary JP2 control, a flattened JPX-brand control and an
independent RGB control, plus blank/rectangle. Pagura passed all 38 controls;
ten subjects were explicit refusals and two (`cgrp-color`,
`combined-first-headers`) were `adapter-error`. Source review attributes the
latter to checking colour completeness before the layer header; the historical
report outcomes are unchanged.

| Observed engine | Matched subject/control evidence |
| --- | --- |
| PDF.js 6.4.299 / Node 26.8.1 / canvas 1.0.11 | Ten complete groups pass. `jpch-palette` and `combined-first-headers` fail despite passing codec/brand/RGB controls. |
| MuPDF 1.28.5 / wrapper 2.0 | Seven complete groups pass. The two palette-header subjects fail with passing controls; opacity nonpasses remain scoped to their paired outputs. |
| hayro 0.8.0 | Seven complete groups pass; palette/premultiplication nonpasses also affect codec controls. |
| PDFium 156.0.8076.0 / pypdfium2 5.14.0 | Four complete header/identity-registration groups pass; opacity nonpasses also affect controls. |
| Poppler 24.02.0 / PyGObject 3.48.2 | RGB controls fail, so the subjects do not establish layout-specific negative results. |
| Quartz macOS 26.6.2 build 25G83 | Flattened JPX-brand controls fail; layout-specific negatives remain unadjudicated. |

Saved pixels confirm that PDF.js/MuPDF render the two failing palette-header
subjects as raw-index grayscale `(0,0,0)` through `(3,3,3)` while their flattened
controls apply the palette. T.801 M.9.2.7/.8 and M.11.6 still make that metadata
applicable. The independently authored expectations remain unchanged. The five
malformed vectors and one unadjudicated chroma-key vector are outside this
render diagnostic. Family comparison cells remain unprobed.

| Artifact | ID | ZIP SHA-256 | Report SHA-256 |
| --- | --- | --- | --- |
| Ubuntu report | 12685997189 | `760877ec515a37b35c7147949e67a4ef5f183d593f533c4a4f1bca51aedcbb80` | `0203a6cdac75c3315e81898e3615bcee1ac61fba5f1ec7d02920cb34f3151a9f` |
| macOS report | 12686036998 | `0b857196bcf57bfad2b80977f88ea6286e847c51818c99cbd5717d79867001e0` | `39eef77bee10408571784844499d1b5414c33cf39889fd6a72c81af3df5e5d7e` |
| Ubuntu pixels | 12686187159 | `d7242931e0a8d42129adba6c8b4c83364c82c477218c9ceca7f6d092d04e7118` | — |
| macOS pixels | 12686362765 | `3302cb27d29e81df555e24ee109d913d1dc178c46ab69250c92f4fc4d13876d7` | — |

The measured Pagura binary SHA-256 is
`07ea943d75a6f5dd35a40cb421ce884cc4e1460883e64d6df3456291ec9fe0ab`.
Platform reports retain the complete engine/build/adapter identities and
other-platform `not-run` rows; Linux failed on Pagura's expected missing cases,
so combined-report jobs were skipped.

The implementation keeps one box parser and one retained `Header`. It resolves
first-image defaults, codestream fields and layer fields before checking their
completeness, preserving ordinary JP2 error order. Explicit locations prevent
duplicate first-image ownership; empty inherited headers differ from missing
required fields. The selected layer accepts identity registration against
codestream zero. Later-image headers remain bounded, irrelevant metadata when
they are not dependencies of that layer.

Opacity types 0/1 become the existing channel definitions after the palette's
final channel count is known. The same `ChannelsUse` and PDF Indexed/SMaskInData
paths preserve raw samples, alpha precision and premultiplication. Four final
channels charge exactly 24 bytes for definition storage and five work units (one box plus
four initialized definitions); one-short controls refuse before allocation.
Identity registration reads ten bytes, with a nine-unit refusal. No extra
sample pass, decoded plane, colour engine or external-data provider is added.

All 54 diagnostic records retain their exact files/oracles. Two further paired
groups cover dimensions supplied by `jpch` and unrelated later-image overrides.
The area now has 62 records: 56 render PDFs, five malformed vectors and one
unadjudicated vector. The full manifest
has 388 observations, including the six historical malformed colour groups
qualified above. Public raw-sample tests, native/WASI/browser-shim checks and
reference conformance use the existing paths. Hosted acceptance of this new
implementation is pending.

The JPX requirement preserves the T.801 M.11.7.6/Table M.34 field-presence
contradiction (and the repeated Otyp=0/undefined PR wording in Table M.33).
Single-byte opacity types 0/1 are supported; chroma-key is explicitly
unadjudicated, with no guessed pixel/error oracle. Selected-image fragments,
cross-references, nonidentity registration and unimplemented required reader
features remain explicit gaps.

## R02/R03 internal JPX fragment diagnostic

`tools/fixturegen/jpx_fragment_probe.py` authors six subjects with the existing
codec/RGB oracle owners. Product source is unchanged from `20011ead5`. The
private rendering manifest contains 26 cases: each subject has an ordinary JP2
control, a flat-jpxb control with the same codestream, and an independent RGB
control, plus blank/rectangle. Tolerances stay one channel unit for encoded
images and zero for RGB controls. Three malformed vectors and one arithmetic
Limit boundary remain outside that rendering denominator. No hosted result is yet claimed.

T.801 M.9.2.5/.6 permits ordered in-file fragments for the selected codestream
and referenced metadata. M.11.3.1/Table M.17 defines the fragment count and
64-bit offset, 32-bit length and 16-bit data-reference fields. Every reference
here uses DR=0. Metadata references precede the selected codestream, and the
JP2 header precedes all media/first-image headers (M.9.2.6/.7). M.11.4 resolves
referenced box contents rather than searching for a physical target header;
the split `cgrp` case deliberately has no such contiguous target header.

The subjects cover a single codestream span, three spans splitting marker
bytes, referenced `ihdr` contents, split `cgrp` child-box contents, combined
metadata/codestream fragments, and a later fragmented image that PDF does not
select. Deliberate gaps between Media Data boxes prevent a reader from treating
the bounding byte range as the concatenated stream. Selected fragmentation
requires Cor.4 feature 13 in DCM/FUAM; later-only fragmentation and the second
layer are FUAM-only. The three malformed vectors isolate an offset outside the
file, an extent beyond the file, and a truncated list count. The finite offsets
and lengths use 4096 so they cannot overflow WASI32. A separate u64::MAX+1
extent requires Limit under CONTRIBUTING, including address-space narrowing.

Dispatch only the existing `capabilities.yml` workflow on the reviewed private
ref, using its explicit GitHub-hosted runner labels. Retain platform reports,
actual pixels, source/binary/engine identities and artifact hashes. Adjudicate
all four roles together; an adapter error is not automatically Unsupported.
This diagnostic does not cover external providers, out-of-order data, recursive
references, binary-filter offsets, or family-wide support. Its reduced manifest
must never replace the retained PR's full image controls.

The original sRGB `cgrp` subject is intentionally interpreted only as a
file-level output observation: an implicit RGB guess gives identical pixels.
The follow-up `reference-split-linear-icc` group distinguishes that case using
one restricted matrix/TRC ICC record. The existing independent profile builder
and scalar IEC sRGB transfer give pixels `(0,137,255)`, `(255,188,0)`,
`(137,225,99)`, `(188,99,225)`, rather than the stored RGB values. Subject,
ordinary JP2 and flat-jpxb roles have identical profile/codestream bytes; the
JP2 reserved colour fields remain zero and JPX APPROX is one. The reference
splits the child-box length field and has no physical contiguous target header.
All original 26 render rows and 28 fixture records remain byte-identical; the
extended private manifest contains 30 PDF cases. This additional control awaits
hosted evidence and does not change any product behavior or tolerance.
