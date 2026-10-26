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
inspections. The DNL constant control requires 81 operations, 384 live bytes and 64
pixels; one-short bounds refuse it.

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
headers, first-image jpch/jplh/opct metadata, ordered in-file fragments and
cross-references, CIELab 14/default or explicit parameters, e-sRGB 20, ROMM-RGB 21,
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
