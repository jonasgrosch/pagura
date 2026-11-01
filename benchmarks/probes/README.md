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

[Hosted run 40331344005](https://github.com/jonasgrosch/pagura/actions/runs/40331344005)
passed at `5def96197e4e8ae3118f5a8f3aacc0e845e2fd3e`: 143 service observations
passed and four text controls were explicitly adapter-unavailable (two each for
hayro and CoreGraphics). Pagura passed all 21 selected independent expectations.
The run also passed the source checks on all three platforms and the unchanged
six rendering controls. These are revision-specific observations, not a parity claim.

[Hosted interoperability run 40333905010](https://github.com/jonasgrosch/pagura/actions/runs/40333905010)
passed at `9eb83d924bb3d5f81e2d82831a5f4b7fadc13969`: Rust linting, public
library/CLI/host service tests, native/WASI byte parity, viewer source selection
and attachment-password retries, independent saved-file readers and browser
thumbnail pixels. Full functional and comparative reports identify their own
source revisions and remain separate evidence.
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
