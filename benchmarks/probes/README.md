# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
embedded-file bytes and text** using independently authored populated/empty
controls and a non-PDF rejection control. All six baseline columns are retained.
G01 remains open: these ten cases do not establish family or product parity.
The companion [render manifest](rendering.json) contains 352 independent pixel
oracles for basic graphics, JPX and stored/generated annotation appearances. Its
2,464 cells use the public Pagura CLI and all six existing public render adapters. The ledger retains six competitor columns;
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

## Consolidated appearance probes

The appearance feature PR owns all subtype, border and text render controls.
Stored/generated counterparts, 72/144 DPI, clipping, opacity, view flags and font
identity remain explicit. Independent geometry/colour/glyph calculations supply
expectations; reader artwork differences are observations awaiting adjudication.

[Hosted run 40340802030](https://github.com/jonasgrosch/pagura/actions/runs/40340802030)
measured merge `444760a59d27dfa185448c41d59d6ea59ee030d5` from head
`e3ac6a553b39de407c9937d73833d7ae28fba2d6`: 411 passing cells and
1,332 pixel disagreements. Pagura passed all 249 controls. Combined report artifact
`12659673555` has SHA-256
`f2edde2c3d2249213e9e96c880594b0b57e61c536d57b0f7c432d18956338d87`.
This does not validate every annotation variant; native/WASI/client and
save/reopen checks remain separate. No source-family comparison is promoted.

The following checkpoint added 57 controls for cloudy/rounded fill, stroke and dashes,
unitless letter spacing, and an ordinary curved-path miter. The Linux report from
[run 40350317258](https://github.com/jonasgrosch/pagura/actions/runs/40350317258),
head `6fc72d23247cb3522a8b652e831bdfdda406c40d`, recorded 298/306 passing Pagura
controls and eight dashed-curve disagreements. Artifact `12663743485` has SHA-256
`f8cfec878599c0b7d3129e7dfa9269a8a8e12c4c36b7224ae4ac47b530fcc7da`.
The combined report was skipped; this is not a completed seven-engine result.

The shared stroker now retains accepted source-curve controls only for dashed
strokes, measures bounded user-space arclength, and evaluates the original curve
and tangent at each cut. The follow-up [run 40354436956](https://github.com/jonasgrosch/pagura/actions/runs/40354436956),
head `0c4ac72b177cb205c209bc9071a23e34375e4ef3`, passed 305/306 Pagura controls.
Its Linux report artifact `12665528340` has SHA-256
`2f10a15176e39f09da62422f90668a2c39b15fbad823712e1185f1fa9d92c97b`.
The lone short circular dash at 144 dpi exposed a preceding chord-normal strip
protruding beyond the final butt-cap plane. The shared run now retains smooth
source tangents at dashed subdivision vertices; a noncircular quadratic pixel
control and exact circular trapezoid check accompany this correction. Rich
controls use embedded literal Type3 glyphs, so character expectations do not
depend on external font substitution. Analytic curve normals, miter geometry and
an exact circular-dash trapezoid establish stroke expectations; their pixel
oracles and tolerances are unchanged.

[Hosted run 40357415540](https://github.com/jonasgrosch/pagura/actions/runs/40357415540)
measured merge `f3174f872c245459c236b285e530cb835af2e54d` from retained head
`85486f465e7ca2294c9313558138875ef0988cdc`: all 306 Pagura controls passed,
within a completed 2,142-cell seven-engine report. Combined artifact
`12667638001` has SHA-256
`f00d715b913d0e85ae660f0ccf426d99e22215926d3c1128be1f6e0273540805`.
[Native/WASI/client and save/reopen checks](https://github.com/jonasgrosch/pagura/actions/runs/40357415274)
and [full Ubuntu/macOS/Windows checks](https://github.com/jonasgrosch/pagura/actions/runs/40357415839)
also passed at that retained head. These results cover the stated fixture set;
they do not validate the later screen-placement batch or complete V01.

## Static annotation screen placement

The initial 46 pixel controls and 12 refusal/error controls in
[annotation_display.py](../../tools/fixturegen/annotation_display.py) distinguish
ordinary page content, stored Sound appearances, ordinary Watermark appearances
and `FixedPrint` screen placement. [Hosted run 40365754193](https://github.com/jonasgrosch/pagura/actions/runs/40365754193)
at head `52cbbce87b3d35d77757d2ea085daf0e04609258`, measured merge
`76a765502e7e2ffe72e1d16d8e171ecd76a6af8b`, passed all 352 Pagura controls in
2,464 recorded cells. Combined artifact `12671551174` has SHA-256
`d4d7b7f64e26d5ee154462e15ca3ffa67bd608e0b4f6d9f1aade99a4ff9d1ca3`;
its report file SHA-256 is
`1a83867dbad74b8cc2d6aa966cb3bad063731274d50c52abddac33fed7d65090`.
This does not complete V01 or prove physical print behavior. ISO 32000-2
12.5.6.22 explicitly uses MediaBox dimensions for screen display. The existing
appearance fitter uses the upright box of the origin-translated Rect after the
FixedPrint Matrix; the shared page-view mapping locates its media anchor and the
existing NoRotate view cancels page rotation. UserUnit and ordinary display zoom
remain intact. CropBox clips this placement and does not redefine its media.

The source AP has a black left half and red right half in an 8×4 box. For
MediaBox `[10 -6 42 18]`, H=.5 and V=.25, the independently authored y-up
display coordinates are:

| Page Rotate | Displayed media | Target x,y,w,h | CropBox `[13 -4 37 13]` viewport | Cropped target x,y,w,h |
|---|---|---|---|---|
| 0 | 32×24 | 16,6,8,4 | 24×17 | 13,4,8,4 |
| 90 | 24×32 | 12,8,8,4 | 17×24 | 10,3,8,4 |
| 180 | 32×24 | 16,6,8,4 | 24×17 | 11,1,8,4 |
| 270 | 24×32 | 12,8,8,4 | 17×24 | 7,5,8,4 |

The four crop offsets are respectively `(3,2)`, `(2,5)`, `(5,5)` and `(5,3)`;
the 90-degree control clips one unit of the red edge. UserUnit2 on the 90-degree
page doubles media to 48×64 and target to `(24,16,16,8)` before DPI scaling.
At 144 DPI the NoZoom control instead keeps artwork 8×4 while its ordinary
media anchor moves to `(32,20)`, yielding target `(32,16,8,4)`. Literal rectangle
oracles never call the product mapping. Rotated ordinary AP counterparts prove
that absent FixedPrint retains normal page rotation; separate AP Matrix and
transparency-group controls exercise the same form path.

Required-Type errors, malformed Matrix/H/V, indirect values, null defaults,
reversed Rect, negative fractions, irrelevant cyclic entries, hidden/text-only
short circuits, numeric overflow and existing reference/scene budgets accompany
the pixel cases. Static painting never reads the Sound payload. Speaker/Mic
artwork is reader-chosen; the following matched observations precede the original
product symbols and remain distinct from their independent oracles. PDF.js canvas
display omits its HTML annotation layer; CoreGraphics drawing omits PDFKit.
Those configurations cannot establish full-viewer icon support. No adapter here
requests playback or physical printing. Printer media/fit-to-paper scaling,
nonprintable margins, tiling and n-up remain V05 contract and probe gaps.

The existing harness also has a separate `observe` command for the three
missing-Sound-AP inputs plus blank, ordinary-content and stored-Sound controls.
It records normalized RGB hashes, dimensions and y-down nonwhite bounds, with
source/fixture/adapter/native identities. Named explicit refusals remain
`unsupported`; other nonzero exits remain unclassified `renderer-error`
observations, with diagnostic hashes. Blank output is an observation, never a
feature verdict. The raw reports have kind `raw-render-observations` and are not
added to oracle pass/fail totals or accepted by the oracle-report combiner.
Hosted actual images remain artifacts and never become product icon oracles.
The capability workflow's manual `sound_observations` input runs just this
diagnostic batch on explicit Ubuntu/macOS runners, without repeating the full
platform, service or pixel-oracle suites.

The Linux report from [run 40365811257](https://github.com/jonasgrosch/pagura/actions/runs/40365811257),
head `52cbbce87b3d35d77757d2ea085daf0e04609258`, records identical ordinary and
stored-AP controls across PDFium, MuPDF, pdf.js, Poppler, hayro and Pagura.
MuPDF 1.28.5 and Poppler 24.02.0 draw distinct Speaker and Mic substitutes, with
absent Name identical to Speaker; their extents and artwork differ. PDFium
156.0.8076.0, pdf.js 6.4.299 Node canvas and hayro 0.8.0 produce blank missing-AP
output under the recorded configurations. Pagura explicitly reports
`annotation.appearance`. The completed CoreGraphics continuation is described below.
Linux artifact `12669564145` has SHA-256
`00adefa059f6a738fde18fd610c434e42abb783d9daf9a39310bf661ef629fc3`;
its report file SHA-256 is
`86bfaa8310e3b4d37afa2051dd7202416efbffcc991c1d0ed53969a30c317288`.
Raw images are in artifact `12670491376`; they supply observations, never copied
product artwork or independent icon expectations.

The 33-case Linux continuation [run 40367784509](https://github.com/jonasgrosch/pagura/actions/runs/40367784509),
head `92d29ab9607fe22702f4245f8c259fd3a5e7dbb2`, adds 26 matched Sound/Text
missing/stored-AP views at 72/144 DPI, Rotate0/90 and explicit NoZoom/NoRotate
combinations. The original three subject PDFs and control hashes are unchanged.
Report artifact `12671905928` has SHA-256
`3a41529cc8ec2a3efd2948ed02f7ec9fd58852c17616489a101bcd88f1c5276c`;
report file SHA-256 is
`6944d5f9b95741848d5d3ef512d6bb8274aab791cda751ea77a3f375becc2b0e`.
The paired macOS report records CoreGraphics on macOS 26.6.2 painting ordinary
page content while leaving every Sound/Text annotation blank, including stored
AP controls. This establishes the scope of the `drawPDFPage` adapter, not a
PDFKit/viewer limitation. Its report artifact `12671676873` has SHA-256
`1c79432343aea8d827d54d51cc4656dee0cd65faa32326b9ece74b1bf49af664`;
report file SHA-256 is
`53a8892feef47db1f06f833523386fbb20c569b448ff020ce7ba372451ded792`.
Raw observations never contribute to pixel pass/fail totals, and renderer
differences do not promote a family.

Both MuPDF and Poppler scale missing Sound icons with DPI and rotate them with
the page when F=4; their Text controls remain upright on rotated pages. For
example, MuPDF's Speaker nonwhite bounds are `[12,12,22,22]` at 72 DPI,
`[24,24,44,44]` at 144 DPI and `[4,24,24,44]` at 144 DPI/Rotate90. Poppler's
corresponding bounds are `[13,12,19,16]`, `[26,25,38,32]` and `[16,26,23,38]`.
All bounds are half-open device coordinates measured from the top left. All
five baseline adapters also scale and rotate the stored Sound AP normally.
Explicit NoRotate makes MuPDF/Poppler Sound upright. Their DPI export adapters
scale Text and explicit-NoZoom stored controls too, so these observations do not
establish interactive viewer zoom behavior.

Clause 12.5.6.16 describes Sound as Text-like in most ways; the unconditional
NoZoom/NoRotate rule is explicit for Text in 12.5.6.4. Pagura therefore keeps
ordinary Sound placement, consistent with the two observed static fallbacks,
and honors explicit Sound flags through its existing fixed-view contract.
Text's existing fixed icon view and stored-AP precedence remain unchanged.
No subtype-specific view or transform path is introduced.

The Sound implementation adds original rectilinear Speaker/default and Mic
symbols to the existing framed-icon owner. Its 34 new pixel controls use
independently authored unions of rectangles: each orthogonal stroke expands by
half a unit, miter corners meet at the offset-line intersections, and open ends
stop at the butt plane. Stored counterparts fill those regions in one operation;
generated counterparts stroke polylines. Exact quarter-pixel coverage and
source-over formulas are shared with the file-icon oracle, whose existing
fixtures and expectations remain byte-identical. The controls cover both icons,
72/144 DPI, rotation, explicit view flags, color, opacity, null/default/unknown
and indirect names, hidden/NoView, stored precedence and ignored cyclic payloads.
Malformed visual values and reference/content/scene budgets have separate checks.
The new G01 set has 386 oracles; this Sound increment still awaits hosted runtime
validation. Custom artwork variants, interaction and playback remain outside
these static drawing claims, and the V01 family remains partial.
