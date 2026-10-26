# G01 capability probes

The first executable slice probes **page counts/dimensions, Info metadata,
embedded-file bytes and text** using independently authored populated/empty
controls and a non-PDF rejection control. All six baseline columns are retained.
G01 remains open: these ten cases do not establish family or product parity.
The companion [render manifest](rendering.json) retains six original pixel
oracles: blank and rectangle controls, fractional geometry, JPX grayscale and straight/
premultiplied JPX alpha. Nineteen legacy font controls extend the manifest to
25 cases and 175 cells, using the public Pagura CLI and all six render adapters. The ledger retains six competitor columns;
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

## Legacy post-name inputs

The font group adds seven independent rendering controls to the six original
controls: three post 2.5 subjects, three matched post 2 cases, and an ordinary
rectangle with the same analytically defined glyph area. Outlines, advances and
cmap bytes match within each font pair. The routes are direct Unicode cmap,
name fallback after an unmapped cmap code, and an explicit PDF Differences name.
The original synthetic rectangle supplies every pixel expectation.

[Hosted diagnostic run 40381321934](https://github.com/jonasgrosch/pagura/actions/runs/40381321934)
at `cce9569c5e53ead8d42fcfc9c045931acfa633f6` used a reduced nine-case manifest.
MuPDF passed all nine cases. Pagura passed all six controls; its three subject
failures remain `adapter-error` in that older harness, which did not retain a
named refusal. The source reader rejects the post 2.5 version. PDF.js and Poppler
passed the direct subject and its pair, but missed the two name-based subjects
while passing their post 2 controls. PDFium and CoreGraphics missed paired font
controls; hayro missed the name-based controls. Those cells do not establish
format-specific gaps. No tolerance was relaxed or measured artwork copied.

Linux report SHA-256:
`73beefea8ecaaafd6cb908477975d46c9cf795ed0dd3dab6130f533a375aacfb`.
macOS report SHA-256:
`76dee296a926897956bc6973451ee1907820a13aedf103d50b0f27930e58b43f`.
The integrated 13-case manifest preserves the original six controls. At `6ac4b1d27caa9bf2690f1105764b027059932998`,
[hosted run 40385518384](https://github.com/jonasgrosch/pagura/actions/runs/40385518384)
passes all 13 Pagura cases. Its combined report contains 91 cells; SHA-256
`a61e91bceb3d01c4bb2aab579788179647b942e653f3b237044adedc3b5dba66`.
The retained diagnostic failures describe the previous reader. The same integrated
head passes [full Ubuntu, macOS and Windows checks](https://github.com/jonasgrosch/pagura/actions/runs/40385513689).
Neither result establishes the entire font family or validates later changes.

The preceding vertical-substitution checkpoint `a0533c2d6` passed
[full hosted Ubuntu, macOS and Windows checks](https://github.com/jonasgrosch/pagura/actions/runs/40380082421).
That validates its native synthetic maps, PDF metrics/extraction and bundled
collection controls at that revision; it does not establish competitor-family
parity or validate the later base and post-name combination.

## Legacy post 4 admission

The integrated set now retains all 13 prior controls and adds four exact
post 4/post 3 direct-cmap subjects/controls from diagnostic `791f19b6c`. The
[Linux diagnostic](https://github.com/jonasgrosch/pagura/actions/runs/40387744536)
shows MuPDF, PDF.js and Poppler passing both subjects and all controls; Pagura
passes its five controls but refuses both subjects. PDFium misses the paired
font controls; those cells are not format-specific evidence. Report SHA-256:
`5ca216fa6965417fe0332667edcb9ada956e7a949a135be70fd17c204d02c8f7`.

A [separate six-name probe](https://github.com/jonasgrosch/pagura/actions/runs/40390798842)
at `dd0a67d3c` compares hexadecimal spellings with literal post 2 name controls.
MuPDF, PDF.js, Poppler and hayro miss all six post 4 names while accepting every
paired name control. The primary text does not fix case or padding; these are
observations, not six normative obligations. Report SHA-256:
`63825a1f18748072b40ba9a13f4778bdaeaec173f6de38aab23b01cd98c9a77d`.

The existing post reader now admits the complete code array for cmap use, with
no synthesized printer names, copied metadata or alternate selector. Exact
count, truncation and work-budget controls pass the
[three-platform functional run](https://github.com/jonasgrosch/pagura/actions/runs/40393127365)
at `b1fd431a8`, together with all 17 rendering controls. No family support is claimed.


## Legacy cmap numeric-code lookup

The eight integrated format-8/format-12 PDFs retain the bytes and independent
rectangle oracles from diagnostic `e8ffe19a1`. The two supplementary pairs
map the literal PDF byte A to a different curved glyph, preventing byte fallback
from accidentally passing the expected supplementary mapping. Earlier nonisolated
subjects remain in their diagnostic branch and are not counted as isolated
supplementary evidence. Packed-surrogate candidates remain observations there;
this reader API looks up an already delimited numeric code unchanged.

[Diagnostic run 40393193630](https://github.com/jonasgrosch/pagura/actions/runs/40393193630)
records 21 cases. MuPDF passes both BMP subjects and their format-12 controls,
and the isolated Windows UCS4 subject/control; its isolated packed-UTF16
subject fails. Its platform-0 supplementary control fails too, preventing a
format-specific negative there. Pagura before the implementation passes both
isolated format-12 controls but fails their format-8 subjects. PDFium, PDF.js
and hayro also fail paired font controls. Poppler's platform-0 isolated
format-12 control passes while its format-8 subject fails. These scoped
observations do not establish engine-wide absence or font conformance.
Linux artifact `12686106513` has ZIP SHA-256
`a9e7ad3da9b26fc702e7b3e325f62ace290ed31ce5e44a663bda706b72d5d346`
and report SHA-256
`bd01f3e8635b336fe37ff184ad401b5caba157a25413fa606c26fe2005d8223f`.

The preceding [17-control post4 render run](https://github.com/jonasgrosch/pagura/actions/runs/40393132189)
passes at `b1fd431a8f2e7709b8a356064c21e382c2415117`, with 119 cells.
Artifact `12685656614` has ZIP SHA-256
`956d550a6fdfdcff527e8e768ec2d876a1576e73475bd3dd9fcf940ed3da6c58`
and report SHA-256
`8c98d6aa0ab8c81f872c0f5c52f0ea7ff42a95b9f899b55213c98084667c02ec`.
The subsequent cmap implementation at `e399327d78c1a1c302235977cfb9a5532c88a759`
passes all 25 Pagura controls in [hosted run 40395913198](https://github.com/jonasgrosch/pagura/actions/runs/40395913198),
with 175 cells. Artifact `12685388918` has ZIP SHA-256
`83f2818c0602e4514f86f570fe62f9c03d77e4ad98df2714e649cd56118f766b`
and report SHA-256
`25f988c14b8dc61ca4d94f06eed99ef08c618f0a609b28bff7a236dcf3d126fe`.
All prior controls and tolerances are preserved. The corresponding
[functional run](https://github.com/jonasgrosch/pagura/actions/runs/40395908628)
passes on Ubuntu, macOS and Windows.


## CFF2 DICT operator scope

[Diagnostic run 40397598168](https://github.com/jonasgrosch/pagura/actions/runs/40397598168)
at `fdfc47c1e` retains 12 cases. Pagura passes eight controls and refuses four
matrix subjects. Hayro passes the paired CID control and applies the legacy
matrix in two isolated cases, producing the independently authored 20×40
rectangle. Other baseline engines miss the paired CFF2 control; their subject
results do not establish a matrix-specific gap. MuPDF and PDF.js pass the CFF1
control with an explicit Top matrix. The omitted-Top CFF1 observation remains
unresolved and is excluded from the integrated acceptance set.

Current [CFF2 FontDICT](https://learn.microsoft.com/en-us/typography/opentype/spec/cff2#fontdict)
and [stack decoding](https://learn.microsoft.com/en-us/typography/opentype/spec/cff2#stack-based-cff2-decoding)
rules require unknown operators to discard their operands. The existing parser
now distinguishes that context, ignoring deprecated FontMatrix without changing
CFF1 or Top DICT matrices. Legacy matrix application remains a separate
compatibility question because both interpretations use the same version-2 header.
The same source's [DICT data paragraph](https://learn.microsoft.com/en-us/typography/opentype/spec/cff2#dict-data)
calls behavior for an operator outside the current namespace unspecified.
Pagura follows the explicit stack-decoding instruction; this internal wording
conflict prevents treating that interpretation as an unambiguous conformance
verdict for every producer or reader.
The original diagnostic reports, fonts and pixel expectations remain unchanged.
Nine of those PDFs/controls join the preceding 25 integrated controls. At
`95249301d`, all 34 Pagura controls pass in [hosted run 40403532318](https://github.com/jonasgrosch/pagura/actions/runs/40403532318),
and [full functional checks](https://github.com/jonasgrosch/pagura/actions/runs/40403532292)
pass on Ubuntu, macOS and Windows. The combined report covers 238 cells at
merge `2f89cbc28efafa544ce6c0a50ea2552886b724b4`; artifact `12690375705`
has ZIP SHA-256 `a1b179444cdc51d405f656c682c70fbfc00825e7fa136325cd62ceafc5c0f826`
and report SHA-256 `760e50b5a5d106b05f5e72d884f0744464c69641b3cb1051a62087128b5a86a5`.

Linux artifact `12687168595` has ZIP SHA-256
`763a9df7cff0b36950ebcb6ee43a15efbc987f0d0f94c25c97131b6649c8f8c0`
and report SHA-256
`ea7a6a90c330062312b7aa3da5f1a6987b4e26098dcca590df6a033d30982369`.
The native `cff2-font-dict-matrix` expectation changes from the historical refusal
to its independently authored untransformed outline, with identical font bytes.

The same current-spec discard rule also applies to the distinct Top and Private
DICT namespaces. One predicate in the existing parser now covers all three,
before entry allocation and duplicate-key validation. Repeated foreign keys
therefore cannot trigger CFF1 synthetic-font or width behavior. Four additional
native fonts and eight simple/CID PDF controls preserve a literal 100-by-200
outline; at 50 points their expected rectangle is 5 by 10 pixels. They bring
the integrated set to 48. These new fonts omit FDSelect for their single
FontDICT, as required by the current specification. All font bytes, records and
pixel expectations published through `3e5d8e69c` are unchanged. The eight new
controls await hosted acceptance. Required CFF2
structures and complete known-operator validation remain explicit review gaps.

## Type 1 advance vectors

Adobe Type 1 section 6.4 defines both coordinates of `sbw` and requires a
`seac` composite's opening metrics to match its base glyph. The existing
charstring state now retains both advance coordinates; the horizontal metrics
API still returns the x coordinate. PDF text placement remains governed by
the PDF widths and text matrix.

Eleven native controls cover signed and vertical advances, exact composite
metrics, malformed operands and width-only decoding. Six independently authored
PDF controls compare horizontal, upward and downward advances under ordinary
and quarter-turn text matrices. Literal bearing/rectangle geometry establishes
the pixels. They bring the integrated rendering set to 40 while preserving all
34 earlier controls, their bytes and their tolerances. Checkpoint
`3e5d8e69cc7e04e5730a7b6a12b583c63684e789` passes
[all three functional platforms](https://github.com/jonasgrosch/pagura/actions/runs/40408412284)
and [all capability checks](https://github.com/jonasgrosch/pagura/actions/runs/40408412177).
The combined report records 40/40 Pagura observations and 280 engine cells at
merge `2ee47ec3610c20fda7e51b6ef22859a18b6f12cc`. Artifact `12693725018`
has ZIP SHA-256 `acd3178e1d7c6c64f628015d0365e4633779e86343f19f808bfe826380ffee68`
and report SHA-256 `fc175b33ad985248f7593f771b1af751ded43eee9216ca4cf1d8251f1afdeb96`.
This evidence precedes the additional eight DICT controls and current one-pass
base changes. It does not extend the static PostScript program subset.


## Font collections

[Diagnostic run 40413575630](https://github.com/jonasgrosch/pagura/actions/runs/40413575630)
at `4aa3d18ce2c9c1f5d35353150eff29bab5188f80` uses 22 controls: four ordinary
pixel controls and three embedding routes with standalone first/second faces,
single-face TTC versions 1/2, and a shared two-face collection naming either
face. Routes are simple/CID TrueType and CID OpenType CFF. The original writer
uses literal 100-by-200 and 200-by-400 glyph rectangles; all pixel tolerances
were fixed at zero before the run.

pdf.js 6.4.299 passes all 22, including selecting the second named face.
MuPDF 1.28.5, Poppler 24.02.0 and hayro 0.8.0 render the first TrueType face
when the second is named: the observed RGB hash exactly matches the first
rectangle control. MuPDF's CFF standalone controls pass but its CFF collections
are blank. Poppler/hayro CFF standalone controls also fail, so those results do
not isolate collection support. PDFium 156.0.8076.0 and Quartz 26.6.2 fail the
standalone font controls; their collection failures likewise establish no
format-specific verdict. All engines pass the four ordinary controls.
Pagura passes the standalones and ordinary controls; all twelve collection
subjects return adapter errors. The runner did not classify these as
`Unsupported`, and the diagnostic outcome remains unchanged.

The Linux report artifact is `12694308182`, ZIP SHA-256
`21a7c12923ac925c9fd60f9ac8f2c709321fcbd8fae3955bb431691bfea71b97`,
report SHA-256 `6545d699412607421b07cd2fdf820b42ec56bf6a51e7ffe359022d09e4313811`.
The macOS report artifact is `12694732307`, ZIP SHA-256
`0c9c8f4c484a4b43f6d24c5512e7eb3d580730b5a286cd35a22f3b2fa0160771`,
report SHA-256 `61b6c7ff919800ea4003d1b16ee42b078365c7fc417b34c6b2c292ee3008577f`.
Only each report's selected engines are observations; the other rows are
placeholders. Linux's expected Pagura failures prevented combined-report jobs.

The implementation reuses the existing sfnt directory and glyph readers,
retains original shared bytes, and selects a unique face from ordered exact
PostScript name hints. Native callers can also use an index. PDF name binding
is an explicit reader policy because Table 124 supplies no collection index.
The original twenty collection-area records and all PDF/PPM bytes are unchanged
in the integrated set of 68 controls. Ten native vectors add shared-owner,
index/name, late-directory and inert DSIG coverage; unit tests exercise
name-table versions/encodings, ambiguity, truncation and budgets. Hosted
implementation acceptance is pending. No family comparison is promoted.
