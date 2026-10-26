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
The capability workflow's manual `annotation_observations` choice selects
`sound`, `redact` or `none`. Either annotation set runs only its diagnostic batch
on explicit Ubuntu/macOS runners; `none` runs the ordinary inventory, service
and pixel-oracle suites. The CLI's `observe` command keeps Sound as its default;
`--annotation-set redact` selects Redact preview inputs.

Oracle runs use the same named-refusal parser. A verified Pagura CLI tag or
structured backend refusal is recorded as `unsupported`; other failures keep
their unclassified error outcome. Pagura still requires a pixel pass in every
oracle case. Earlier reports retain their original classifications.

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
The 386-control set passed [hosted run 40371829262](https://github.com/jonasgrosch/pagura/actions/runs/40371829262)
at retained head `8dd6279e93e02ed969fed2576f964448428d5d2f`, measured merge
`6bdb9f119fe57e9deb28ac9d6dcb6e828aa07607`, with 2,702 recorded cells. Combined
artifact `12673292246` has SHA-256
`266c11f89d18ad24fe8526047b0e4e6d6f1e625f5e56af39d81c54a31fc0fbf9`;
report file SHA-256 is
`a7b6a3900c9f74fd22133fb114c0000a334cf57569fcff924b9af72c2c73d31c`.
[Native/WASI/client and save/reopen checks](https://github.com/jonasgrosch/pagura/actions/runs/40371829227)
also passed, as did [full Ubuntu, macOS and Windows checks](https://github.com/jonasgrosch/pagura/actions/runs/40371829208). All twelve gates passed that retained head. The subsequent Redact batch is recorded below.
Custom artwork variants, interaction and playback remain outside these static
drawing claims, and the V01 family remains partial.

## Redact preview observations

ISO 32000-2 12.5.6.23/Table 195 separates non-destructive identification from
applying redactions. QuadPoints identifies the intended region; absent quads
use Rect. IC, RO, OverlayText, Repeat, DA and Q describe replacement artwork
after content removal, and RO is distinct from normal annotation AP. These
entries do not require a static display renderer to remove content or draw
post-application replacements. Sanitizing removal remains group 16.

The first bounded diagnostic has 18 raw cases: blank and cyan/black ordinary
page content, plus missing/stored normal AP pairs for Rect, two quads, border
colour, interior colour, quad/interior combination, overlay text, repetition
and a replacement RO form. Overlay inputs carry an original embedded Type3
glyph and explicit resources; the RO form has distinct green/blue artwork.
Every stored normal AP has black/red artwork, so its precedence can be observed
separately from replacement entries. The underlying page content remains in
every annotated PDF. No redaction is applied and no audio, actions or external
resources are used. The source fixture coordinates and PDF identities are
retained with each raw report; no expected preview pixels are copied from a
reader.

[Run 40373036796](https://github.com/jonasgrosch/pagura/actions/runs/40373036796),
source `10445c5367162afb1aea989596d63e0d4d5dd886`, completed both platforms.
MuPDF 1.28.5 draws a red missing-AP preview; changing C, IC, overlay text,
repetition or RO preserves the corresponding Rect/quads preview hash. All five
Linux baselines and Pagura paint the same stored normal AP. PDFium, pdf.js Node
canvas, Poppler and hayro leave missing-AP inputs identical to ordinary page
content. CoreGraphics leaves both missing and stored annotation inputs identical
to ordinary content, establishing its adapter boundary. Every invoked baseline
case returned observed pixels, including the RO case; no renderer error is
interpreted as preview absence. The raw report retains all engine versions,
adapter settings and binary identities and never supplies expected artwork.

Linux report artifact `12674435567` has SHA-256
`fb30292c02264445aa13f2d4b6dcba7b874112784b096d91175543c5da6cc972`;
report file SHA-256 is
`e2032ad43835d681e2d6842b27d382a3d59eb6fe9011d7c311ed307534beaa08`.
macOS report artifact `12672309093` has SHA-256
`3f3d708c97af94db76d814b9486842198d4ba6427b44d3d72bd60b7b6a0bdc6e`;
report file SHA-256 is
`85e89932b90468dea449f8907e9d067b553596847cf84bb9d74933dee19819af`.
The 18 raw cases stay outside oracle pass/fail totals. All original diagnostic
PDFs, Sound subjects and previous pixel oracles are unchanged.

Pagura's explicit preview policy uses one-unit red outlines of QuadPoints, or
Rect when quads are absent/null, clipped to Rect. It shares the text-markup quad
reader, operation/storage/segment limits and Marks writer; it adds no renderer,
font, replacement-form or content-removal path. The quad reader follows Table
182's counterclockwise order; MuPDF's crossed outline for the authored quads is
an observed ordering difference, not a geometry oracle to copy. Existing
alternate-order repair reporting and strict rejection remain intact. All quads
are one stroke, so overlapping outline regions receive opacity once. The shared
closed-path writer closes each subpath at the next moveto, using its existing
stream-byte budget; open paths retain their original output. C concerns
popup/icon colour here and is ignored by this red-outline policy; CA controls
the stroke. IC, RO, OverlayText, Repeat, DA, Q and nonstroking ca are not read for
the outline, including malformed or cyclic values. Stored normal AP wins even
when substitute-preview geometry or replacement entries are malformed.

Thirty new pixel controls use literal offset contours, rational winding/clip
areas and source-over equations over cyan/black page content. A sloped quad has
edge direction (3,4), so its outward half-unit offset is (2/5,-3/10). Intersecting
these supporting lines with y=7.5/16.5 yields outer vertices
`(7,7.5),(20.25,7.5),(27,16.5),(13.75,16.5)`; the inner vertices are
`(9,8.5),(19.75,8.5),(25,15.5),(14.25,15.5)`. Stored equivalents fill those
independent contours instead of stroking the product path. The controls cover
72/144 DPI, rotation, clipping, overlapping quads/CA, null and indirect defaults,
hidden/NoView and stored precedence. An original Type3 glyph remains visible and
extractable; integration checks retain its Unicode/position and all original
file bytes before and after preview rendering. The preview never removes data.

Separate error controls reject non-array, empty, short, nonnumeric, clockwise
and concave QuadPoints; malformed CA; cyclic quad references; and overflowing
orientation arithmetic. Existing shared limits also bound Rect fallback and
explicit quads, including repeated scene recording under a tight byte budget.
The resulting G01 set has 416 oracles. At `b042f0230d6cb7380b86c68f86b663c329eda740`,
[hosted capability validation](https://github.com/jonasgrosch/pagura/actions/runs/40379150530)
passes all 416 Pagura controls (2,912 recorded cells across seven engines).
The combined report SHA-256 is
`4876aa9666344b6f5682344ed10e16fad64094dfa8884a2c7db4d760a80d4df0`;
artifact 12677898081 has ZIP digest
`93f320dc8847cda4c431042f720155ad1caf139772d8dece21da84d514ffe8a6`.
[Editing interoperability](https://github.com/jonasgrosch/pagura/actions/runs/40379145451)
also passes that head. These results precede integration of one-pass base
`9f9801a75`; that combined source needs fresh hosted validation. This increment
does not implement applied redaction or promote V01.


## Source widget display

The `widgets` raw observation set retains its 13 missing/stored Tx/Ch pairs,
blank and literal ordinary-content controls. PDF 1.7 inputs distinguish source
values from deliberately different black/red stored artwork. They cover
inherited/null attributes, NeedAppearances, multiline/comb, MK rotation,
combo/editable/list choices, multiselection and TI, with text at 72/144 DPI.
The embedded original Type3 font has 600-unit advances and 500-by-700-unit ink;
choice export and display strings differ, and V uses Table 234 display names.
All 27 original PDFs and 14 original pixel controls retain their exact hashes.

[Hosted observations](https://github.com/jonasgrosch/pagura/actions/runs/40380559074)
at `68fa04401a095b61d2139a5a8aab3fb735f852f6` show MuPDF 1.28.5 drawing all
13 missing appearances; Poppler 24.02.0 draws all except the noneditable combo.
MuPDF, Poppler and pdf.js replace the stored artwork when NeedAppearances is
true. Otherwise pdf.js 6.4.299 leaves missing appearances blank in the existing
Node canvas adapter; Hayro 0.8.0 leaves all missing cases blank and retains stored
artwork. PDFium 156.0.8076.0 with annotations enabled and Quartz CoreGraphics on
macOS 26.6.2 omit even stored widgets while drawing the ordinary rectangles.
These omissions are adapter observations, not full-viewer results. Pagura's
pre-change build refused the 13 missing cases with `annotation.appearance` and
retained stored artwork. Differences in font fallback/layout are observations,
never sources for the generated pixel oracle. No action or event is invoked.

The Linux report SHA-256 is
`e8fc0c9dc34f7708cf10b95de4e0ec493ad09f53a984dd63af7e5d9bd5669494`
(artifact 12678567119, ZIP digest
`1ccb80f7a4810a80479f3198017698e72f34b0a1ee6a073b546cbc2fa836f83a`).
The macOS report SHA-256 is
`90f3ea10723d0e670dc3b79e2e9d1a666df8527f9a3498245909dbe4c6be87f1`
(artifact 12678027211, ZIP digest
`685c9f3c58be0b8d5390377053f095316062409bb3314faef31ab4eb55d5d732`).
Reports preserve renderer/configuration identities and every subject/control
hash. Images remain hosted artifacts, separate from independent oracle totals.
The existing `observe --annotation-set widgets` command reproduces this scope.

The source path uses a bounded requested-widget projection in the document form
owner: selected Parent ancestry must agree with Fields/Kids membership, and the
owning field must be terminal. It reuses the full view's attribute, flag, type
and value readers without building all fields/names or reading actions and DV.
Unnamed separate widgets use their parent's field attributes (12.7.4.2); MK is
widget-local (Table 192). One borrowed input feeds the existing variable-text
font/layout, style, resource, MK rotation and appearance serializer for source
and filled values. No per-page FormFiller, borrowing cache or second renderer
is introduced. Existing traversal/depth/result-memory and generated-content/
scene budgets apply; no speed claim is made.

Ordinary display retains a selected stored AP and does not execute the
NeedAppearances request. Untouched missing Tx/Ch previews use source values.
Explicit specification previews/saves may regenerate for NeedAppearances, using
the existing Tx replacement/append rule; unmarked stored artwork remains.
No-op host saves retain original bytes. Hidden/NoView, stored AP and text-only
paths stay ahead of source lookup, and generated widget glyphs do not become
extracted page text. The full AcroForm API keeps its existing inspection/repair
contract; the narrow source view refuses inconsistent membership rather than
searching unrelated field trees to repair backlinks.

Forty-seven independent controls bring G01 to 463 cases. The 48-by-32 widget
has a two-unit inset. At size 10, ascent/descent 8/-2 and advance 6 give baseline
13 locally (21 on the page); right quadding starts at x=42 and center at x=26.
Four comb cells are 11 units wide, so centered AB starts at x=23.5 and 34.5.
Multiline baselines are page y=30,18. List rows start at page y=38 with height
12 and baselines 29,17,5, clipped to [10,10,54,38]; independent selected-row
rectangles use RGB (0.6,0.75,0.9). Two separate widgets on one field have page
baselines 13 and 29. Literal rectangle transforms cover MK rotation, page
rotation, UserUnit, NoRotate and NoZoom; the latter anchors the upper-left corner at
(16,80) at 144 DPI while retaining one-unit glyph geometry. The NoRotate control maps its
upper-left corner to (24,56) on a quarter-turned page and keeps the glyph
baseline horizontal at y=45. Rational polygon
coverage computes these pixels without parsing an emitted appearance program.

Controls include default/null/stream V, separate-widget field overrides,
authoritative V versus inconsistent I, unused font/action/default entries,
stored/hidden laziness, zero area and a stored ordinary-art-plus-text control
for explicit regeneration. Malformed selected DA/Ff/V, options, Comb, I/TI,
missing/repeated membership, Parent cycles and nonterminal owners fail
explicitly. Rich text and XFA retain named refusals; broader CID/complex-script
layout, push-button artwork, actions and physical printing remain boundaries.
Cross-crate checks compare source display, untouched preview, same-value saves
and strict reopen with these pixels, retaining original bytes/values. The host
loop checks native/WASI/browser equality, tiles and no-op reopen. Runtime
validation of this widget batch is pending; the preceding 416 oracle definitions
are unchanged, and V01/V02 remain partial.

[Hosted editing run 40389416478](https://github.com/jonasgrosch/pagura/actions/runs/40389416478)
exposed an incorrect new pixel expectation for `widget-source-null-normal`.
ISO 32000-2 Table 170 requires N in a present AP dictionary; 7.3.9 makes a null
N absent. That same PDF is now a Syntax control, consistent with the existing
shared-selector test. Optional AP=null still permits generation. The correction
removes only that invalid pixel case (464 to 463); product code, all remaining
pixels/tolerances and the previous 416 controls are unchanged.

The completed [0309 capability run](https://github.com/jonasgrosch/pagura/actions/runs/40388042315)
records 463/464 Pagura passes; that null-N case is its only failure. Its Linux
report SHA-256 is `fe33ae63738c7b5cf8cf84db4b2835341406c8ec13ba3565571fbcd02def4774`
(artifact 12682810693, ZIP digest
`5fe74e97c3f06806a8a568b02f23a58d32af967f88844080c45fe89a8cba2b98`).
This is exact old-head pixel evidence, not a functional or final-head acceptance
claim; #256's corrected integrated checkpoint has fresh hosted gates pending.

## Normal button display diagnostics

The `buttons` observation set adds 31 paired missing/stored normal appearances,
four ordinary-content controls and the existing blank control. It covers
checkbox/radio on/off and disagreeing AS/V, explicit captions, NeedAppearances,
push-button caption/icon arrangements TP 0–6, transformed Form XObject icons,
SW A/B/S/N with smaller/larger icons, proportional/anamorphic fit, alignment,
border/full-bounds fitting, MK/page rotation and 72/144 DPI. Every push-button
input omits V and DV. No edit, action, rollover or down event is requested.

Primary ISO 32000-2 12.7.5.2.3–.4 gives AS precedence over disagreeing V;
12.7.5.2.2 gives push buttons no permanent value. Table 192 defines normal CA
for all button kinds and I/IF/TP for push buttons. Table 250 defines icon scale
conditions, aspect-ratio policy, proportional alignment and border fitting.
Exact caption font fallback, partition sizes and padding remain reader layout
policy to adjudicate from scoped output, not a universal pixel requirement.

The normal stored artwork is two literal rectangles at page (12,12) and
(24,12), each 8 by 10; its on/push colours are black/red and its Off colours
blue/green. The ordinary caption uses the original 600-advance, 500-by-700 Type3
glyphs at size 10 with page baseline 21 and x=10,16. The ordinary icon is black
over [8,8,16,16] and red over [16,8,24,16]. A transformed icon has source
BBox [2,3,18,11] and Matrix [0,1,-1,0,11,-2], giving upright bounds [0,0,8,16]
before placement; it is distinct from its stored-AP control. Literal polygon
coverage supplies only the stored/ordinary controls, without calling product
layout or parsing generated appearance programs. Existing Sound subjects,
Redact/widget diagnostics and the 463 existing pixel definitions are unchanged.

Hosted observation run [40391658709](https://github.com/jonasgrosch/pagura/actions/runs/40391658709)
measured exact head `02df2700baadbb6e1bac32237fd00809f4177618`. All six
baseline adapters produced all 67 requested views without adapter failures.
MuPDF drew 28/31 missing-AP views and Poppler 27/31; the three Off states were
blank, and Poppler's default radio was blank while its explicit radio caption
drew. pdf.js drew five checkbox views. Hayro's missing views were blank.
MuPDF, Poppler, pdf.js and Hayro drew all 31 stored-AP controls. PDFium and
Quartz also left every stored widget blank while drawing all ordinary controls,
so those canvas configurations cannot establish whole-viewer widget absence.
Pagura at this diagnostic head recorded 36 controls and 31 named refusals.

MuPDF and Poppler generated byte-identical caption-only output for the normal
push caption, TP 1–6 and all icon/fit variants; their ordinary icon controls drew
both colours. These observations support caption/state generation in these
configurations. **They do not demonstrate push-icon synthesis.** Icon fitting
in this batch follows primary Tables 192/250 and independently authored
geometry; no hosted baseline image becomes a product icon or pixel oracle.
AS over V and the adapters' differing NeedAppearances policies remain recorded
separately. Pagura keeps stored button AP during ordinary source viewing and
explicit form preparation; only absent normal artwork is generated.

The report records MuPDF 1.28.5 (PyMuPDF wrapper 2.0), Poppler 24.02.0
(GI 3.48.2), pdf.js 6.4.299 (Node 26.8.1/canvas 1.0.11), Hayro 0.8.0,
PDFium 156.0.8076.0 (wrapper 5.14.0), and CoreGraphics on macOS 26.6.2
(build 25G83). Configurations are unchanged from the adapter descriptions above:
PDFium annotation drawing, pdf.js display canvas without an HTML widget layer,
and Quartz CoreGraphics without PDFKit. Raw outputs remain separate from
pixel acceptance and family support. Reproduce with
`annotation_observations=buttons` or `observe --annotation-set buttons`.

Provenance: Linux report artifact `12684921993`, ZIP SHA-256
`64b94fc128767c9111f97b1242ade8abab303813d62f50dc40f0829c3e2e5d28`,
report SHA-256 `f7692218a1112be30b50309c7c311561f6199cb44f678e51d77a73b2191a067a`;
macOS report artifact `12683291607`, ZIP SHA-256
`9f3225322fd6148f6343b0d7fb1c456ae8b2640ec3768f85248e0beb07f26588`,
report SHA-256 `83706f75e3f0adec3fff49e2a15774ef01175b92ce1f930716291ac0d8baad2e`.
Image artifacts are `12684672155` (Linux) and `12685190151` (macOS). The
source PDFs and their identities are unchanged; the former Pagura refusals
now have renderable diagnostic expectations without a universal artwork oracle.

### Shared normal-button implementation and pending acceptance

The same requested-widget projection and normal form generator now serve
source display, untouched filler preview and explicit regeneration/save.
Captions use the existing DA/font/plain-text owner. Default toggle marks retain
the existing ZaDb policy. Push icons remain source Form references and execute
through the ordinary interpreter; the shared appearance owner holds the one
BBox/Matrix parser, upright bounds and fitting transform. Their former
interpreter-local definitions are removed. Actions remain inert.

The 85 new pixel controls bring G01 to **548**, preserving all prior 463
identities, pixels and tolerances. Expected caption ink comes from the original
Type3 widths and rectangles. For the 48-by-32 widget, the 16-by-8 icon fits to
48-by-24 at page (8,12); unscaled fitting gives 16-by-8 at (24,20). The transformed
icon has upright bounds 8-by-16 and fits to 16-by-32 at (24,8), with black below
red. Ordinary Form Matrix/BBox controls use these geometry facts without an
annotation. Caption/icon equal-half partitions and two-unit text padding are
explicit reader policy. Border fitting, mixed-dimension anamorphic scaling,
all TP positions, alignment, rotations, separate/inherited fields and AS/V
selection have independent controls. Sixteen malformed input cases, ignored
entries, text-only laziness and existing program-byte budgets are also covered.
Push saves produce AP/N streams with no V, DV or AS; toggle saves preserve both
states and validate an unselected malformed caption atomically. Missing AS with
Opt compares V with the widget ordinal; duplicate export strings do not equate
distinct on-state names. Explicit shared AS names remain shared for checkboxes
and radios in unison. Numeric, shared-name, null-AS and stored controls verify
source/preview/save selection, including persisted Off rather than dangling AS.
The requested view reuses its existing Kids membership; the filler uses its
existing grouped widget index. An exclusive multi-widget radio with non-Off V
but no AP, AS or Opt refuses `annotation.appearance` rather than guessing the
selected child; an explicit widget edit resolves that ambiguity. Off groups
remain renderable. Untouched
no-op saves remain byte-identical.

The preceding retained head `5321c0958726c2174b1e0e1518debf34dee738e7`
passed hosted functional run `40392132368` on Ubuntu/macOS/Windows and editing
run `40392132561`. Capability run `40392132510` measured merge
`2963e8c1e748f1e11b2c4ca71b6a95922b5905b7`: **463/463** Pagura controls,
3241 combined cells; artifact `12685018627`, ZIP SHA-256
`a9299cef5f4286592a5cd721631d9d6e38ad33d18569b6bec9f73d8591c44485`, report
SHA-256 `e4013329a413e2a1b39b8276ee7c878b5851999b03f0259e9670525d62e69f18`.
That evidence does not validate this new normal-button source checkpoint.
The new batch awaits hosted acceptance; no local build, test or engine was run.
R/D activation, richer form data/actions, rich/CID/complex-script captions,
physical print and broader V01/V02 closure remain outside this batch.

Hosted editing run `40404196880` at retained head `4b42c5b43` exposed one
authored oracle omission in `button-normal-opt-stored-control`: its first
widget explicitly selects the original coloured Off stream, while the oracle
included only the second widget's selected black caption. The PDF is unchanged
(`6423293a228803fd2ee58a9ff9f2432e015eb123c4068d07ef588d67b86e3bbd`).
Fitting Off's 48-by-32 BBox into `[8,8,28,40]` gives `x'=8+5x/12`, `y'=8+y`.
The corrected independent oracle therefore includes blue at `(29/3,12)` and
green at `(44/3,12)`, both `10/3` by `10`, plus the existing black 5-by-7
caption at `(43,21)`. No product code, tolerance or other oracle is changed by
this adjudication. The original failure remains in the hosted run history.

At `358abe709`, [functional run 40405543406](https://github.com/jonasgrosch/pagura/actions/runs/40405543406)
identified four missing informational expectations in the original button
observations. The on-state check/radio cases without a caption use the existing
ZapfDingbats mark policy and the explicit standard-font provider, so they report
`font.substituted`. Their fixture records now require that information; PDF
bytes, pixel oracles, tolerances, repair expectations and product code are
unchanged. Stored appearances and original Type3 captions retain no such report.

### Final static subtype boundary

ISO 12.5.6.18 defines Screen without AP as blank, even when MK/I is present.
Absent/null AP therefore returns no artwork; an existing AP still wins and
malformed selected AP/N remains Syntax. Movie `Poster` (13.4, Table 306) is
absent/false by default, an explicit image XObject, or true to request a poster
from the movie file. Only the explicit image case generates artwork. It keeps
the original image reference in an ordinary Form and uses the existing image
interpreter; `Poster=true` retains `annotation.appearance` Unsupported. Movie
files and all activation/actions remain inert.

Movie Rotate is normatively clockwise in multiples of 90. The existing shared
quarter-turn frame implements it. Fitting the explicit image to Rect is declared
reader policy; Table 306 describes Aspect as the movie's pixel dimensions and
provides no required poster-fitting algorithm. Ordinary image and stored-vector
controls independently establish the same placement. The original 2-by-2 image
has black/red top samples and green/blue bottom samples; its literal quadrants
cover all rotations, two DPIs, crop, UserUnit and explicit NoZoom/NoRotate.
Image XObject colour spaces are direct PDF objects (8.6.3, Table 87), so generated
resources need not inherit page colour names/defaults. A resource-only colour
name is a malformed selected poster control. Decoder/colour/mask capabilities
and limits remain those of the existing image path.

3D (13.6.2), RichMedia (13.7.2), PrinterMark (14.11.3) and TrapNet (14.11.6.2)
require normal AP. Stored artwork/state controls paint without resolving unused
payloads; absent/null AP is Syntax after ordinary visibility and text-only
culling. They do not represent a generic appearance-synthesis backlog.

The 72 new cases include 48 pixel controls, bringing G01 to 596. They are
independently authored acceptance fixtures, not measured competitor artwork. They await the retained PR's hosted gates. Existing Sound,
Redact, widget and button observations and all prior input identities are retained.
The advertised static boundary uses one stored selector, generator and interpreter.
Richer font/encoding coverage depends on group 04; rich form editing and complex
Tx updates on group 07; media decoding/activation, XFA and host interactions on
group 08; physical print on group 10; sanitizing redaction on group 16. Remaining
common-rich-text layout and difficult geometric accuracy cases stay explicit V01
gates, without promoting this finite static boundary to full V01 acceptance.

The rich Tx RV/DS batch extends the same parser and layout used by RC annotations
through source display, explicit library filling, preview and incremental saves
(ISO 32000-2 12.7.4.3/Table 228 and 12.7.5.3/Table 231). `FillValue::RichText`
retains inert bounded XML and an optional replacement DS in the document layer;
the appearance layer validates the selected XML/formatting/field constraints.
Saving obtains plain V from that same parser and writes V/RV/optional DS plus
the completely regenerated normal appearance in one transaction. Unchanged
stored AP, no-op saves, DV, R/D and unrelated source entries are preserved.
DA/Q inherit; RV/DS remain local to the terminal field. DS overrides the DA
base style; nested run styles override DS. No script, XFA form program or
external resource is evaluated.

Independent controls use the existing authored Type3 font: 0.6 em advances,
0.8/-0.2 em metrics and 0.5 by 0.7 em ink. With Rect [8,8,56,40] and two units of
padding, the single 10 pt baseline is 21. A 20 pt run moves the common baseline to 18;
two 10 pt paragraphs have baselines 30 and 18. The secondary F2 font has a distinct
0.9 em advance and 0.7 em ink width, proving that run font selection affects both
ink and following placement. Ordinary and stored controls paint literal colored
rectangles; none interprets generated output. The 90/180/270-degree controls use
independent literal rectangles, and Comb uses rational cell centers. Existing
one-level glyph coverage bounds remain unchanged.

The original `widget-source-rich` PDF remains byte-identical: its V=AB/no-RV
combination now reports Syntax, following Table 231 bit 26's required RV for a
valued RichText field. Empty/no-value absence remains valid. Malformed or
unsupported selected XML/DS, DoNotScroll, single-line, MaxLen, password masking,
reference/work/storage limits, ignored source RV/DS under stored AP, full old
BBox/Matrix/resource replacement and atomic save failures have focused controls.
Plain Text replacement is explicitly refused; browser rich editing input,
complex-script/CID/font coverage, advanced rich measurements and automatic sizing
without a positive DA/DS/run size retain their existing host/font/layout boundaries.
This batch is source-verified only until the existing PR's hosted gates run;
paired baseline outcomes remain scoped to each configured render adapter.

The sealed rich-field set contains 61 cases: 48 pixel controls and 13 explicit
selected failures, bringing G01 to 644. All 596 earlier pixel definitions remain
unchanged; only the non-pixel valued/no-RV classification is corrected above.

### V02 artwork-preserving text and choice updates

ISO 32000-2 12.7.4.3 requires existing resource names to win DR collisions and
replaces Tx marked content, appending when no Tx exists. The library retains
that policy. The browser's explicit `PreserveExisting` policy removes unmarked
show operations while preserving surrounding artwork. Both now use the normal
content reader's exact source spans and the accepted image owner's
`inline_data_length_with_resources` framing; no second tokenizer, image framer,
layout or interpreter is introduced. Group 03 checkpoint
`42795ad266d7d7173563b4257fcbce8007470957` is an explicit prerequisite, privately
merged as `4dd6846bdc14c0aa879a0d0eaaa7bdec82215bae`. Its source and controls are
prerequisite changes, not additional group 07 implementation.

Thirteen subjects have distinct ordinary before/after controls (39 pixel
cases). Their authored Type3 glyphs advance six units at 10 pt; OLD occupies
5 by 7 rectangles at x=10,16,22 and y=11. New AB has the same ink at x=10,16
and y=21, centered from the 8/-2 metrics in [10,10,54,38]. A repeated Tx section
under a literal ten-unit translation moves its replacement baseline to 31.
The resource-collision subject instead uses 2.5 by 5 ink with the same advances,
so replacing the existing font from DR would visibly fail. Red image/Form
artwork occupies [10,32,16,36]. Five literal gray inline samples spell `x EI `,
including a false delimiter, and each occupies two by four units. These exact
rectangles independently define all expectations at 72/144 DPI; generated
appearance bytes and baseline-renderer output never supply oracle pixels.

The set covers raw, named-colour and filtered inline framing, Image XObjects,
recursive text-free Forms, Tx replacement/append, repeated balanced Tx sections,
choice display values and font-resource precedence. Seven selected-update
failures cover unmarked Form text, clipping text, cross-BT/q scopes, truncated
inline data, non-stream resources and a Form cycle. Unchanged no-op saves stay
byte-identical and do not select those payloads. Edited previews/prepared
streams, strict saved reopens, DV and alternate R/D preservation, repeated saves,
reset and atomic failure retain one generation/persistence path. Token, storage,
scope-depth and graph-walk limits use the existing budgets.

The 919 existing annotation PDF/oracle files and all 1,026 combined G01 probe
definitions are unchanged. The new set brings this private combined manifest
to 1,065 controls and awaits hosted functional, native/WASI/host, save/reopen
and cross-reader validation. Pixel observations measure configured rendering;
they do not establish another reader's editing behavior. Ambiguous unmarked
Form text and Tx scopes crossing BT/q remain explicit refusals. Broader font
coverage, data exchange and a flatten API retain their separate group 07/04
boundaries.

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
