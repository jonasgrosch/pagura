# Plan to close competitive PDF feature gaps

Status: proposed implementation plan, reconciled against `render/one-pass` at
`19059e68448288ccad84a3bc2f4c6c69c5142e2f` (PR #118).
The documentation reconciliation and G00 source inventory are delivered: 90 tag
candidates reviewed, all registered requirements assigned, and six baseline API
catalogs reconciled across 43 capability families in the
[checked capability inventory](CAPABILITIES.md). G01 runtime probes are in
progress; later packages remain open. Component names identify ownership, not
assigned people. The follow-up starts from `render/one-pass` at
`7c03b656d1e1093c6951e80ec2fc189b5b1bf90a`, which includes the G00/initial G01 merge.
[STATUS.md](STATUS.md) records what exists now.

## Baseline and consolidation

Implement against the one-pass architecture, then its merge into `main`, not the
older `main` renderer. Before starting a package, record the then-current merge
or branch revision and recheck its affected capability rows. The source revision
above is the audit baseline, not a claim that PR #118 has merged.

The documents have distinct roles:

| Document | Role |
|---|---|
| [STATUS](STATUS.md) | Current supported layers, boundaries and source evidence. |
| This plan | One competitive backlog: package IDs, dependencies and acceptance gates. |
| [ROADMAP](ROADMAP.md) | Milestones and priorities pointing to this backlog. |
| [ADR 0034](adr/0034-one-interpreter-one-renderer.md) and [architecture](ARCHITECTURE.md) | Accepted one-pass component and accuracy contracts. |
| [Async loading and image streaming plan](plans/async-pdf-loading-and-image-streaming.md) | Detailed P0–P9 implementation design for D01 and D02a's image/output slice. Its packages are subdivisions, not a second delivery queue. |
| [Proposed ADR 0035](adr/0035-progressive-sources-and-streaming-jobs.md) | Pending source/job API decision for that slice; consolidation does not accept it. |

Preserve what one-pass already delivers: `pagura-content` tokenization,
`pagura-interpret`'s single interpreter and negotiated `Device::Needs`, separate
text extraction, `pagura-appearance`, cached device-independent scenes, source-order
preparation, per-buffer precision and one band executor in `pagura-paint`.
`pagura-render` stays a thin orchestration layer. Exact is the only rendering
mode; neither `Accuracy::Competitive` nor the removed compositor routes are
implementation targets. Extend existing devices and shared preparation instead
of adding a second parser or renderer.

Public `RenderContext::render_bands`, between-band stop callbacks and CLI streamed
PPM output already exist. D01/D02a extend these with persistent resumable jobs,
range input, incremental PNG and host backpressure; basic band streaming is not
missing work. The earlier async proposal has been updated to this distinction.

## Target and completion rule

Match the **union of PDF capabilities actually provided by the six established
baselines**, at recorded versions and configurations: PDFium, MuPDF, pdf.js,
Poppler, hayro and Apple CoreGraphics (ADR 0011). A feature implemented by only
one applicable baseline still creates a parity obligation. Compare rendering,
reading, interaction, editing, export and validation separately; a renderer-only
baseline is not required to edit, and an adapter limitation is not an engine
limitation. CoreGraphics is the baseline; PDFKit is not silently substituted.

This is a versioned target, not a promise about every future competitor release
or every optional Adobe extension. Adobe automation remains outside the existing
baseline decision. PDF/A and PDF/UA validation are additional M8 obligations,
even where the six engines do not provide complete validators.

Completion requires all of the following:

1. Every discovered capability has a ledger row with a normative locator or
   documented interoperability contract, responsible component, work-package
   ID, applicable baselines, configuration, fixtures and expected behaviour.
2. Every applicable baseline cell is probed. `Unknown`, `not tested`, missing
   adapters, timeouts and unexplained differences remain open; none means N/A.
3. Each required feature works through its intended public API and, where
   applicable, CLI/host/viewer. A decoder or helper alone is not completion.
4. Independent tests establish correctness. Differential output is evidence to
   investigate, not the normative oracle. No unclassified relevant failure
   remains in the pinned generated, upstream, adversarial and real-world sets.
5. The compatibility gate and ADR 0002 performance gate pass separately.
   A narrower release can be named explicitly, but cannot be called full parity.

A prohibited PDF construct, invalid file or documented resource refusal is not
necessarily an implementation gap. Each such classification needs a reproducer
and reason; it must not conceal valid content accepted by a baseline.

## Baseline evidence and missing measurements

These official sources identify comparison surfaces. They are documentation
leads, **not results from the locally pinned binaries**. The source audit did
not run a full cross-engine feature comparison.

| Baseline | Documented surface to probe | Identity and evidence to capture |
|---|---|---|
| PDFium | [Build options](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/README.md) include optional XFA and V8 JavaScript. Probe forms, actions, rendering, extraction and public editing/save APIs. | Wrapper and backend revision, renderer backend, V8/XFA flags and executable/library hashes. Probe the optional capable build separately from the existing benchmark build. |
| MuPDF | [Document API](https://mupdf.readthedocs.io/en/latest/reference/javascript/types/PDFDocument.html), [page/redaction API](https://mupdf.readthedocs.io/en/latest/reference/javascript/types/PDFPage.html) and [write options](https://mupdf.readthedocs.io/en/latest/reference/common/pdf-write-options.html) expose document operations, JavaScript, redaction and encrypted output. | `uv.lock` wrapper plus backend identity, enabled features, host adapter and hashes; do not infer all APIs are available through the current wrapper. |
| pdf.js | [Loading API](https://mozilla.github.io/pdf.js/api/draft/module-pdfjsLib.html) exposes range/stream loading and optional XFA. Test viewer behaviour separately from Node raster conversion. | Node package lock, browser/runtime, Canvas backend, fonts/CMaps/ICC assets, XFA and scripting settings. |
| Poppler | [Document API](https://poppler.freedesktop.org/api/glib/PopplerDocument.html) exposes forms, attachments, layers, metadata, signatures and saving. | Actual system version and available bindings/backends. `pdftoppm` alone cannot establish non-render feature absence. |
| hayro | [Crate API](https://docs.rs/hayro/0.8.0/hayro/) defines a PDF rasterizer and documents its own limitations. | Adapter Cargo lock, crate revision, font provider/features and renderer settings. Verify supported rendering cells rather than treating its goals as implementation. |
| CoreGraphics | [CGPDFDocument](https://developer.apple.com/documentation/coregraphics/cgpdfdocument) is the document entry point used by the Quartz baseline. | macOS/SDK version, Swift adapter, drawing/colour settings and platform-specific applicability. Inventory the actual CoreGraphics API; record unavailable operations explicitly. |

Keep existing pins in `uv.lock`, `benchmarks/node-package-lock.json` and
`benchmarks/adapters/hayro/Cargo.lock`. Record detected backend versions as well.
Refresh pins in a separate reviewed change; never compare a new implementation
against an unidentified or silently changed baseline.

## Delivery order and dependencies

| Phase | Work packages | Exit gate |
|---|---|---|
| 0: establish the denominator | G00, G01 | Inventory and runtime comparison cells exist; current failures are classified and assigned. |
| 1: close page-display failures | R01–R06, V01 | All valid display features found in phase 0 have independent expectations and end-to-end rendering coverage. |
| 2: complete reading and interaction | V02–V05, D01–D03, S01 | Forms, accessibility, layers/printing, large-file loading, document services and signature verification work through public surfaces. |
| 3: complete editing | E01, E02, E03, S02 | Rewrite/edit/encrypt/redact/sign workflows reopen independently and meet preservation or sanitization contracts. |
| 4: standards and release | C01, C02 | Profile-specific gates and complete compatibility/performance evidence pass for the final release candidate. |

Phases express dependencies, not a requirement to serialize independent work.
S01 can start after G00/G01; E01 can start while display work proceeds. V03 and
S02 require design decisions early because scripts and private-key operations
cross existing scope boundaries. No calendar estimate is asserted before G00
sizes the valid, baseline-supported cases.

## G00 — Complete and reconcile the capability inventory

Owner: `references/requirements`, `docs`, `benchmarks`; depends on no new product code.

- **Source inventory complete at `19059e684`:** `benchmarks/capabilities.json`,
  the generated [review view](CAPABILITIES.md), and `tools/capabilities.py --check`
  cover 43 capability families, all 90 reviewed literal tags, all registered
  requirement IDs and all six baseline API inventories. There are 258 unprobed
  family cells. Schema 2 requires dispositions, source locators/hashes, ownership
  and complete surface states; it does not infer runtime support or N/A.
- The review includes dispatch guards (including Huffman JBIG2), dormant tags,
  policy/unknown-input boundaries, actual refusals and untagged service APIs.
  Later discoveries reopen G00. G01 results stay scoped to the executed fixture,
  operation, configuration and engine identity rather than promoting a family.
- The ledger records IDs, operations, source locators, layer states, remaining
  integration, packages, requirement/fixture references and the full baseline
  denominator. G01 owns executable oracles, build/configuration identities,
  per-fixture results and run provenance; their absence never becomes N/A.
- Call-site review followed aliases, helper functions and dispatch branches.
  Dormant tags, malformed-input/policy boundaries and generic image-filter
  dispatch are retained with dispositions rather than counted as missing features.
- Current scope descriptions reconcile security/forms, font substitution, CMS
  modification analysis, host passwords, navigation and interpreter ownership.
  Normative review records remain historical; pending primary-standard review is
  still pending in the owning registry.
- Untagged rows cover attachments/associated files, portfolios, metadata/XMP,
  thumbnails, image extraction, print settings, annotation/page editing, form
  data exchange, accessibility, OCR/tables, signing and vector exports.
- Multimedia/3D, PDF/E/PDF/VT and extension security handlers have explicit rows
  and packages. Their actual baseline configurations and applicability are G01
  probes, including any API/build distinction found during execution.

Acceptance met for the reviewed source revision: every discovered refusal/tag,
registered requirement and discovered baseline API family has a disposition and
owner; no baseline column is silently excluded. This is a versioned source
inventory, not proof that every PDF subvariant or competitor configuration has
been exercised. Later discoveries reopen the inventory and relevant package.

## G01 — Build the feature comparison harness

Owner: `tools/benchkit`, `benchmarks/adapters`, `tools/fixturegen`; depends on G00 schema.

**Started:** the [document-service probe slice](../benchmarks/probes/README.md)
adds twenty-one independent populated/empty/rejection controls, 147 explicit case/engine
cells, identity/source/fixture hashes, and hosted CI. The adapters now cover all
six baselines across hosted Ubuntu and macOS. Hayro catalog/page attachment probes now use public object/stream APIs with a bounded byte-hash bridge; the 21-case hosted matrix is measured below. Hayro and CoreGraphics text reconstruction remain explicit adapter gaps. The wider
operation/configuration matrix remains open. `f948c64f289f2ec80112f92b0f06f7c88b0f882b`
adds Pagura through the public metadata/attachment/text CLI and additive inspect
geometry, preserving all six baseline columns. Every Pagura cell must meet its independent expectation in hosted execution. These
fixture results do not promote any G00 family cell or establish Pagura parity.
The annotation increment adds first-page FileAttachment byte/name probes,
separate from catalog attachments, with mixed slots, duplicate occurrences,
shared/page-only specifications and second-page scope controls.
[Run 40280807877](https://github.com/jonasgrosch/pagura/actions/runs/40280807877)
measured merge revision `c17d3b41a0ed0816b151a0a9380355ce75484be4`: 142 passing
cells, four explicit text adapter gaps and one incorrect observation. Pagura,
PDFium, pdf.js and Poppler pass all 21 cases. Hayro and CoreGraphics each pass
19, with two text gaps apiece. MuPDF passes 20; its binding returns no catalog
attachments for the nested name-tree control. The revised adapter calls MuPDF's native name-tree and embedded-file APIs directly. [Run 40283918313](https://github.com/jonasgrosch/pagura/actions/runs/40283918313), at merge revision `2d10229dff5ac42059128ab8cf079622af5a7f52`, records 143 passes and four text adapter gaps: MuPDF now passes all 21 controls. This resolves the earlier disagreement as a binding-path limitation. All compressed
catalog/page payload controls pass. Later source changes need their own validation.
The initial render slice adds six independent pixel oracles and 42 case/engine
cells, including Pagura through its public CLI, for empty/populated controls, fractional geometry and JPX colour/alpha.
The initial six-baseline run recorded 20 passes and 16 pixel disagreements;
these are observations pending sampling/colour/alpha adjudication, not established
competitor defects. The Pagura-inclusive hosted probe run recorded all six subject cases passing. The separate R02 changes need their own expanded probes after
consolidation onto the shared target.
The annotation render increment expands the shared manifest to 45 oracles and
315 engine/case cells: 39 stored/generated icon, circle/ellipse and InkList
controls with explicit 72/144 dpi, visibility, view flags, geometry and opacity.
Reports pin each adapter's annotation settings. Reader artwork, sampling and
API-layer differences require adjudication; no fixture promotes a family cell.
[Run 40288310225](https://github.com/jonasgrosch/pagura/actions/runs/40288310225)
at merge revision `f7eac50605d5990162ebaf9b5a74009fffb81c5e` records 121 passes
and 194 pixel disagreements, with Pagura passing all 45 controls. Retained images
show artwork, border/inset, overlap-opacity and edge-coverage differences; these
are observations for adjudication, not competitor defect or family-parity claims.

The annotation probe increment expands the shared manifest to 90 oracles
and 630 engine/case cells. Forty-two new observations cover Polygon/Path geometry,
straight Line controls, every non-None ending for Line and PolyLine, signed leaders
and plain captions. Caption comparisons retain the pinned AFM/Arimo identities
for Pagura and each other reader's default font/artwork policy. Existing oracle
bytes and tolerances are unchanged.
[Run 40303222645](https://github.com/jonasgrosch/pagura/actions/runs/40303222645),
measured merge revision `a89009ce9dcc1e18f499860dc76f9743875e38eb`, records
183 passes and 447 pixel disagreements, with all 90 Pagura controls passing.
The combined report artifact is `12641214395`, SHA-256
`922ed9d218fb7a9e3b45e4c2f00daf61f7b33360d4f2cc3f62aaf95d0ffea9d2`.
Font/layout and API-layer differences still require adjudication before capability
conclusions. These source-revision measurements do not validate later changes.

A further 27 text-markup/Caret controls expand the current render manifest to
117 oracles / 819 cells. They include Multiply over black/cyan page content,
overlapping and empty quads, skew, all four markup styles, alternate-order recovery,
both Caret symbols, stored artwork, RD, opacity and indirection. Existing oracle
bytes and tolerances are unchanged. Alternate-order cases measure recovery rather
than strict rendering; their strict_valid metadata is false because the CLI
strict audit includes opening, page enumeration and rendering.
[Run 40310593092](https://github.com/jonasgrosch/pagura/actions/runs/40310593092),
at measured merge `2acedcc7b02b5332a3dd061576dfcf43cbf91447` from head
`1616038d3a7996baa2fcaa981b1f862cdd93dbcc`, records 227 passes and 592 pixel
disagreements across 819 cells; Pagura passes all 117 controls. Combined artifact
`12647349843` has SHA-256 `fea927936968a52fb4b8f8c3ff0b364dd9b012b671ab60ef6558dc0546aa99b9`.
Artwork, recovery and API-layer differences remain subject to adjudication.
The earlier 90-case measurement remains tied to its recorded revision.

The next text-appearance increment adds 63 independent controls for a current
manifest of **180 oracles / 1,260 cells**. All fourteen standard stamps, regular/bold
fonts, plain FreeText alignment/wrapping/auto sizing/DR selection and all standard
callout endings join stored/generated pairs, two resolutions, opacity, rotation,
clipping and empty controls. Reports pin both regular and bold AFM/Arimo assets.
Oracle bytes and tolerances are unchanged. Hosted measurement is pending; no
family cell is promoted and reader artwork/font/API differences still need
adjudication.

Add non-render adapters for extraction/search, forms/events, annotation/layer
state, attachments, save/reopen, redaction and signatures where applicable.
Use browser integration tests for browser-only behaviour, and capable optional
builds when a feature is disabled in a default baseline. Pin options, fonts,
passwords for synthetic fixtures, colour/annotation state and equivalent work.
Keep private corpus data and raw reports outside Git.

Acceptance: every adapter has positive, negative and unavailable-feature controls;
unsupported, incorrect, timeout, adapter failure and limit are separate outcomes.
Independently authored fixtures cover the feature and its important interactions.
Regenerate all committed scoreboards for a fixed source revision, recording
strict/repair opens, rendered pages, errors, timeouts and binary/corpus hashes.
A successful scan alone does not establish visual correctness.

## R01 — Structure, recovery and external data

Owner: `pagura-core`, `pagura-document`; depends on G00/G01.

Reduce residual header/xref/object-stream/revision/page-tree failures in the pinned
suites. Add only bounded, reported recovery with strict-mode counterparts.
Design an explicit host-supplied resolver for external streams and remote document
references; the parser never opens a path or URL. Keep historical views immutable.

Acceptance: all baseline-readable cases in the inventory either match independent
structure/page expectations or have an adjudicated invalid-input disagreement.
Cycles, ambiguous repairs, missing providers and exhausted budgets fail explicitly.
External-resource fixtures prove no implicit IO and stable object/revision identity.

## R02 — Finish JPEG 2000 integration before extending the decoder

Owner: `pagura-image`, `pagura-color`, `pagura-interpret`, `pagura-paint`; depends on G01.

**Started from `render/one-pass` at `7c03b656d1e1093c6951e80ec2fc189b5b1bf90a`:**
`b410d2bcafac55cbe9dbf337dbdbc88144e59ac4` connects embedded ICC profiles to the existing
forward transforms and bounded profile cache. Independent Gray/RGB, alpha,
PDF override and malformed-profile fixtures accompany cache/budget controls;
hosted validation is pending. `0a9f9cb0109d5059350367247212795844a04f7f` adds
eight-bit unsigned sYCC with IEC Annex F rational pixel oracles, explicit PDF
overrides and alpha fixtures. Other/mixed sYCC precisions, Lab-input profiles,
multiple-profile ranking and fallback after an unsupported selected transform remain open. This slice
does not complete R02 or its platform/interaction acceptance gate.
`7040b91024dd10282b4e7d5e51b2e1942ff0b6c4` additionally preserves direct
index/opacity channels for explicit PDF Indexed spaces without a container
palette, with first/last alpha and SMaskInData 0/1/2 fixtures. It rejects signed
sYCC channels before signedness is discarded. The source inventory also includes
`847fb26f0616e8b70bec76883880c78cbc47915d`, which updates the diagnostic formatter
for the new colour model. Hosted verification remains pending.

1. Interpret embedded ICC/sYCC and colour specification precedence/fallback;
   respect explicit PDF colour spaces and expose all relevant colour candidates.
2. Complete Indexed JPX opacity/channel mapping and residual palette/layout cases;
   resolve resource-dependent inline image boundaries without unbounded scanning.
3. Preserve existing straight/premultiplied `SMaskInData` rendering, mixed precision
   and reference-grid adaptation. Extend interaction coverage where G01 finds
   missing cases; do not schedule already-supported embedded alpha as a new feature.

Acceptance: independent sample and pixel oracles cover alpha 0/partial/1,
colour/profile disagreement, masks, groups, interpolation and native/WASI parity.
Decoder-only success and a newly ignored unsupported tag cannot pass this gate.

## R03 — Residual image codec variants

Owner: `pagura-image`, `pagura-codec`; depends on G00/G01 and relevant R02 layout APIs.

Implement baseline-supported JPEG lossless/hierarchical processes, CCITT extension
modes, JBIG2 extended templates/necessary extensions and remaining JPX capability,
precision/layout/progression variants. Obtain and review the corresponding primary
amendments before implementation. Split each codec/variant into its own change.

Acceptance: independent decoder vectors, truncated/hostile cases and PDF-level
fixtures pass with explicit output/work caps. Every valid codec row accepted by
an applicable baseline is covered; an unneeded non-PDF codec feature is marked
as such with a standards/baseline reason, not silently removed.

## R04 — Fonts and text binding

Owner: `pagura-font`, `pagura-text`, `pagura-data`; depends on G00/G01.

Deliver vertical substituted CJK glyph forms/metrics and provider coverage first.
Then close baseline-supported collections, cmap/post formats, variable instances,
CFF2 matrices, synthetic/legacy font programs and nonhorizontal advances.
Review TrueType/OpenType and remaining Type 1 primary sources. Use bounded
font-program evaluation for executable font/CMap cases; general document-script
execution is a separate V03 contract. Treat hinting as an explicit rendering
policy decision under ADR 0013 rather than silently changing exact glyph positions.

Acceptance: independently derived outlines, glyph selections, PDF advances,
vertical placement and extraction mappings agree. Embedded and substituted
variants run with default/no-default/CJK builds, including unavailable-provider
and resource-limit cases. Existing Expert charsets, FDSelect 4 and default CFF2
blends are retained and tested, not reimplemented.

## R05 — Colour management, transparency and overprint

Owner: `pagura-color`, `pagura-interpret`, `pagura-paint`; depends on G01.

Add destination/inverse ICC transforms and output-intent routing; calibrated
blending destinations where permitted; black-point compensation; and complete
fractional, pattern and per-colourant transfer-region selection. Preserve native
CMYK/spot identities through relevant group boundaries. Expose output/profile
and overprint choices through versioned host APIs. Keep prohibited Lab blending
separate from valid missing transforms.

Acceptance: primary-formula colour vectors and reviewed Ghent/PDF Differences
cases cover nested isolated/non-isolated groups, knockout, all blend modes,
soft masks, output intents, spot/process plates and transfer combinations.
Auto/Always/Never overprint paths all meet the same exact-rendering contract;
unsupported profile transforms cannot silently fall back while claiming fidelity.

## R06 — Remaining graphics combinations and accuracy

Owner: `pagura-interpret`, `pagura-paint`, `pagura-raster`; depends on G01,
relevant R02/R05 cases.

Resolve valid singular-stroke/shading transforms, tiling lattice placement,
pattern ExtGState and image-mask patterns. Audit difficult mesh inversions and every operator,
graphics-state parameter, function, clipping and group interaction represented
in the ledger. Preserve exact geometry and the current precision admission plan.

Acceptance: analytic fixtures cover tiny/huge coordinates, repeated partial
coverage, tile seams, nested clips/patterns and degenerate input. Every storage
format stays within ADR 0004's bound through the one executor. Valid numerical
edge cases and malformed/limit cases have separate recorded outcomes.

## V01 — Annotation display and appearance synthesis

Owner: `pagura-document`, `pagura-appearance`, `pagura-interpret`; depends on G01,
relevant R04/R05 work.

**File attachment icons implemented:** `8d758e1c0` generates Graph, PushPin,
Paperclip and Tag when no normal appearance exists (Table 187). These are
original geometric symbols with a yellow default background and black outlines;
unknown names use PushPin by reader policy. Null/absent names use the standard
default. Existing appearances retain precedence, and C/CA/ca style only generated
marks. Forty-two authored fixtures include 39 independent pixel expectations,
stored appearances built from filled regions instead of stroked polylines,
72/144 dpi, rotation, explicit NoZoom/NoRotate, visibility, colors and opacity.
Host checks also compare native/WASI/client and partial-tile pixels. Payload
resolution, authentication and activation remain separate from drawing. Source
checks and hosted native/WASI/client/partial-tile/viewer pixel checks pass at
`c42c6ad8a` in [run 40283824975](https://github.com/jonasgrosch/pagura/actions/runs/40283824975).
The viewer harness compares these oracles at their declared device resolution,
including fixed-size controls; existing tolerances are unchanged.

**Circle/ellipse and rectangle insets implemented:** generated Circle appearances
use eight bounded tangent cubic arcs with independent fill/stroke opacity.
Square/Circle RD entries apply left/top/right/bottom insets before border
positioning, preserving the original appearance BBox. Null/plain BE entries
work; cloudy effects still refuse explicitly. Thirty-two authored controls
include 23 closed-form conic/rectangle pixel oracles, stored appearances,
72/144 dpi, unequal axes, rotation, transparent borders, opacity and indirect
entries. Native/WASI/client, partial-tile and viewer checks use the same
independent expectations. These checks pass at `a69e470c3` in
[run 40286507880](https://github.com/jonasgrosch/pagura/actions/runs/40286507880).
Unequal-axis stroke/dash expectations and richer borders remain open.

**InkList appearances implemented:** samples form straight-connected paths with
BS width/dash, C and CA under the original annotation clip. Reader policy uses
butt caps, miter joins and one stroke operation for all disjoint paths. Repeated
indirect paths count toward aggregate segment/storage bounds; generated content
has its own byte cap. The initial 34 controls include 25 independent pixel oracles
for corners, crossing paths, dashes, diagonals, two resolutions, rotation, opacity,
clipping and stored/generated appearances. Geometry validates even with zero
width or an empty color. Native/WASI/client, partial-tile and viewer checks are
registered and pass at `28e4f3fe5` in
[run 40287592540](https://github.com/jonasgrosch/pagura/actions/runs/40287592540).
The PDF 2.0 Ink Path increment reuses the bounded operator reader. Current
[Table 185 errata](https://pdf-issues.pdfa.org/32000-2-2020/clause12.html) makes
InkList ignored when Path is present: no legacy resolution or allocation occurs.
Null Path falls back to InkList. Rendering does not validate required-key document
conformance. The expanded 51 controls contain 38 independent pixel expectations,
including non-linear circular strokes checked against exact disk areas, straight
cubics, mixed operators, indirection, precedence and malformed required entries.
Hosted native/WASI validation found a 144-dpi curved-stroke oracle disagreement.
Three new G01 Ink Path controls extend the current render manifest to 48 cases
and 336 cells, retaining actual/oracle pixels for investigation. Existing oracle
bytes and tolerances are unchanged. The retained report localizes the failure to
four seam pixels: source Bézier endpoint tangents were lost during flattening.
The repair preserves those tangents for open endpoint caps, including dashes
that reach an original endpoint; native/WASI/client, partial-tile and viewer checks passed at `75c497380` in
[run 40293996890](https://github.com/jonasgrosch/pagura/actions/runs/40293996890).
The 48-case G01 report passes all Pagura controls at that revision; the four
144-dpi seam pixels are now within one level of the independent oracle.
The ignored-legacy controls and all platform suites pass at `c84952094`;
[run 40298054028](https://github.com/jonasgrosch/pagura/actions/runs/40298054028)
records the native/WASI/client and viewer validation.

**Straight Line appearances implemented:** required finite L coordinates resolve
indirectly and validate before empty output. C, CA and BS width/dash use the
existing bounded mark builder and annotation clip; absent/null/None endings and
zero/null leaders are accepted. The initial 41 controls include 22 independent
pixel expectations for two resolutions, dashes, diagonals, rotation, clipping,
opacity, indirection and stored appearance precedence. All 12 hosted checks pass
at `0e8cf8e05`, including native/WASI/client and viewer pixels in
[run 40298058062](https://github.com/jonasgrosch/pagura/actions/runs/40298058062).
The increments below add standard endings and leaders; plain captions are
implemented below. The existing corpus now contains 27 independent pixel expectations.

**Line/PolyLine endings implemented:** all ten Table 179 styles resolve through
LE, including indirect names. Endpoint orientation follows source derivatives,
including repeated curve controls. Closed shapes fill IC/ca; outlines use C/CA
and solid strokes. Reader policy fixes dimensions at three times max(width, 1),
arrow depth twice that, and body/interior/outline mark order. Fixed stack geometry
and bounded content handle zero-width/transparent cases without allocating path
collections. Fifty-one controls contain 47 independent pixel expectations from
analytic offset polygons and exact disk areas at two resolutions; they cover both
subtypes, mixed styles, direction, rotation, clipping, opacity, indirection and
straight-cubic Path endpoints. All 12 hosted checks pass at `6517b1a57`,
including native/WASI/client and viewer pixels in
[run 40298015653](https://github.com/jonasgrosch/pagura/actions/runs/40298015653). The existing Line
arrow and zero-width PolyLine arrow controls now render without PDF byte changes.

**Line leaders implemented:** signed LL, nonnegative LLO and LLE translate the
line proper from the source leader endpoints as Figure 80 specifies. Leader
extensions project beyond the line proper; endings follow its translated
endpoints. Without LL, L names the line proper and LLO has no leader to offset.
Fixed stack geometry validates finite arithmetic before empty output; body and
leaders share one stroke, opacity and dash pattern. Twenty-seven controls contain
21 literal rectangle-union pixel expectations, covering both signs, vertical and
reversed direction, rotation, clipping, coincident endpoints, dashes, opacity,
indirection and ending integration. Numeric overflow and content-budget controls
are also registered. Hosted native/WASI/client and viewer checks pass at
`e746f5e55` in [run 40302043210](https://github.com/jonasgrosch/pagura/actions/runs/40302043210);
all 12 checks now pass at that revision. Diagonal interactions still need
further evidence. The three former leader refusals now
render with unchanged PDF bytes.

**Plain Line captions implemented:** Cap/Contents, Inline/Top positioning and CO
offsets use the line frame after leader displacement. The shared form font encoder
now lives under `fonts`; a font-aware appearance entry point uses the interpreter's
existing provider. Reader policy uses 12-point Helvetica/WinAnsi, centred rows,
1.2-em leading and two units of clearance. Overlong captions move above the line;
inline captions interrupt the body where their metric box overlaps it. Text paints
with C/ca after body and endings, retaining the original annotation clip. Thirty
controls include 22 independent pixel expectations from explicit placements, AFM
metrics and straight contours in the pinned Arimo substitute. They cover two
resolutions, multiline/CRLF text, offsets, reversal, rotation, vertical lines,
clipping, opacity, leader integration, indirection and PDF text-string encodings.
Missing fonts remain explicit in builds without bundled data; empty/inactive
captions need no fonts. Text/work/output and nonfinite-offset checks are registered.
Hosted native/WASI/client, partial-tile and viewer checks pass at `a406c638a` in
[run 40305190338](https://github.com/jonasgrosch/pagura/actions/runs/40305190338).
The harness uses protocol 1.4 full diagnostics on every surface, retaining both
repair and informational assertions. All 12 hosted checks pass at that revision.
Rich text, complex scripts/bidi, broader font selection and unsupported characters
remain gaps; this does not establish full caption or annotation parity.

**Polygon/PolyLine appearances implemented:** finite Vertices or PDF 2.0 Path
commands form bounded geometry in default user space. Polygon closes the path
and uses nonzero IC fill; PolyLine remains open. Path overrides Vertices, with
indirect operands, moveto/lineto pairs and cubic commands. Null/plain polygon
border effects and absent/null/None polyline endings work. Forty-five controls
include 27 independent pixel oracles from convex clipping, literal stroke regions
and an exact quadratic integral, covering curved fills, straight cubic strokes,
closed/open geometry, winding, two resolutions, rotation, opacity, indirection
and malformed data. Native/WASI/client, partial-tile and viewer checks pass at
`0e2c68e0e` in [run 40289812941](https://github.com/jonasgrosch/pagura/actions/runs/40289812941).
Two further controls check ignored malformed and cyclic Vertices under the
current Table 181 errata, bringing the corpus to 47 controls / 30 pixel oracles with the ending increment;
their hosted native/WASI/client and viewer checks pass at `6517b1a57`.
Standard endings are now implemented above;
cloudy borders remain an explicit gap.

**Text markup implemented:** Highlight, Underline, StrikeOut and Squiggly use
finite counterclockwise QuadPoints with indirect arrays/numbers and the first
edge as the text direction. Reader artwork policy fills Highlight with Multiply,
uses a bottom 1/12-height underline, a centred 1/12-height strikeout and a
1/24-height zigzag ribbon. Quads share a single nonzero fill, preserving union
opacity; C and ca (else CA) apply under the original annotation clip. Geometry,
work, temporary storage and output are bounded before empty-color suppression.
Validated zero-area quads emit no marks: the independent empty-region control
exposed device hairlines when such quads were emitted as PDF fills. Mixed
empty/nonempty quads retain the valid geometry; the original oracle is unchanged.
The common alternate TL/TR/BL/BR order uses the explicit `annotation.quad-order`
repair; Strict rendering refuses it. Stored appearances retain precedence.
Forty-one controls include 33 independent pixel expectations from literal polygons,
rational winding/clip integration and blend equations. They cover two resolutions,
vertical/skewed geometry, rotation, overlap, disjoint quads, opacity, underlying
black/cyan page content, indirection, stored precedence and malformed values.
Strict/repair and numeric/resource bounds are also registered. Hosted native/WASI/client, partial-tile and viewer pixel checks pass at
`2c2a2ba54`. The full suites exposed incorrect strict_valid metadata for the
alternate-order controls: the CLI audit includes rendering, so these now require
strict refusal. Product recovery behavior and pixel expectations are unchanged;
all twelve checks now pass at `e18503a20` (functional run 40310188629,
reopen run 40310188609, capability run 40310188768). The 117-case G01
increment includes this subset.

**Caret appearances implemented:** Sy None/P and RD insets use original filled
chevron/pilcrow artwork with no font dependency. C and ca (else CA) style one
compound fill under the original annotation BBox. The shared Square/Circle/Caret
RD reader checks four-element cardinality before resolving fixed stack operands,
requires positive remaining bounds and rejects nonfinite extents/sums. Geometry,
work and output are bounded; unknown Sy names refuse explicitly. Thirty-one
controls include 21 independent pixel expectations from literal page polygons
and rational intersection areas, including direct stored appearances, two
resolutions, unequal/asymmetric bounds, rotation, opacity, visibility, indirection
and malformed fields. Numeric/budget controls and the earlier Circle/Square
checks cover the shared reader. Hosted native/WASI/client, partial-tile and viewer
pixel checks and all twelve hosted checks pass at `b329a075e` after the
markup strict-audit metadata correction (functional run 40310589562, reopen
run 40310589437, capability run 40310589425). The 117-case G01 increment includes this subset.

**Rubber-stamp appearances implemented:** all fourteen standard Name values use
original framed uppercase Helvetica-Bold/WinAnsi artwork through the configured
font provider. Absent/null/custom names select Draft by reader policy. IT defaults
to Stamp; image/snapshot/unknown intents require stored artwork. The smaller
rectangle dimension sets frame width (1/32) and padding (1/8); text fits both
available dimensions using AFM advances/ascent/descent. C defaults to red, CA
styles the frame and ca (else CA) styles the text. Private font resources, fixed
label storage and explicit work/content limits keep generation bounded.
Thirty-eight controls include 33 independent pixel expectations for every standard
name, short/long labels, two resolutions, direct stored artwork, rotation, tall
rectangles, opacity, visibility, indirection and malformed fields. The oracle reads
pinned TrueType points independently and integrates original quadratic curves by
Green's theorem, with analytic clipping/hole/subdivision controls; existing
straight-glyph rational oracles retain their behavior. Builds without font data
explicitly report it. Hosted native/WASI/client, partial-tile and viewer checks
pass at `05dd8d1db` in [run 40311664712](https://github.com/jonasgrosch/pagura/actions/runs/40311664712),
as do all three platform suites and both combined reports: all twelve checks
pass (functional run 40311664668, capability run 40311664531).
The 180-case G01 increment adds the stamp subset.

**Plain FreeText appearances implemented:** required DA, decoded Contents, Q and
RD feed the existing form parser, reversible font encoder and variable-text
layout. Plain and TypeWriter intents use whitespace wrapping, explicit line breaks,
1.2-em default leading and the 12-point auto-size ceiling. Only the selected
AcroForm DR font is retained; an absent exact standard-14 name may use the explicit
provider. Present resources are never replaced and unused resources are not
resolved. Reader policy paints the optional C background, black BS/Border outline
and text clipped inside two units of padding. CA and ca retain their separate
roles; stored appearances win. Fifty controls contain 37 independent pixel
expectations with authored line breaks/sizes, AFM metrics and original quadratic
glyph areas. Controls cover alignment at two resolutions, wrapping, explicit
lines, clipping, auto sizing, colors/opacity, RD, rotation, indirection, DR selection,
visibility, stored precedence and malformed input. Font, work, geometry, storage
and output limits are also registered. Hosted validation is pending. Rich RC/DS,
the form layout's richer font/matrix boundaries remain open.

**FreeText callouts implemented:** optional CL arrays with two or three points
use fixed stack geometry; the scalar LE names the first point's ending. All ten
standard shapes share the existing ending implementation. Inactive or absent/null
CL leaves LE unresolved. Cardinality, finite coordinates/differences and bounded
geometry/work validate before empty text or zero-width output. Reader policy paints
a black body and ending outline, with optional C interior in closed endings,
before the text box; opacity and original Rect clipping remain explicit.
Thirty-one controls include 24 independent pixel expectations from literal
elbow/straight/vertical stroke polygons, analytic ending regions and original
glyph areas. They cover every ending, two resolutions, rotation, separate opacity,
zero width, empty text, visibility, indirection and malformed input. The former
zero-width callout refusal now renders with unchanged PDF bytes. Hosted validation
is pending; wider diagonal/dash combinations still need evidence.

**Rounded rectangular borders implemented:** legacy Border radii now apply to
Link/Square, plain FreeText and supported empty widgets. BS precedence keeps unused
Border unresolved, and null BS is absent. Reader artwork clamps each radius to
half the centreline extent, using two 45-degree tangent cubics per corner; either
zero radius produces sharp corners. Square interiors follow the outline while
FreeText/widget backgrounds retain their rectangular policy. Fixed geometry/work
and output limits protect generation. Thirty-two controls include 28 independent
pixel expectations from closed-form quarter-ellipse/rectangle areas and circular
parallel-set strokes, with separately authored sixteen-arc stored artwork. They
cover two resolutions, opacity, rotation, RD, clamped/zero radii, indirection,
visibility, stored precedence and malformed values. Hosted validation is pending.
Unequal-axis stroke/dash evidence, nonrectangular legacy radius handling and the
separate filled-form border path remain open; cloudy borders are still a gap.
Stored sixteen-arc controls now use Decimal half-angle identities to avoid
platform-libm serialization drift; pixel expectations and tolerances are unchanged.

**Beveled/inset rectangular borders implemented:** BS B/I now synthesize raised
or recessed frames for Link/Square, plain FreeText and supported empty widgets.
Reader artwork uses two bounded facets, interpolating clamped device components
halfway toward white/black and swapping top/left versus bottom/right shading for
inset. Border width clamps to half the smaller extent. Scoped fill-alpha resources
apply CA to facets without changing background/text ca, including opaque borders
after translucent backgrounds. Forty-five controls contain 42 independent pixel
expectations from separately specified trapezoids, exact-area coverage and
source-over, with literal stored counterparts, two resolutions, RGB/Gray colours,
RD, rotation, fractional geometry, opacity, visibility, indirection and malformed
values. Geometry/work/resource checks and native/WASI/client/viewer controls are
registered; hosted validation is pending. CMYK conversion expectations,
nonrectangular relief, nonempty/rotated widgets and the separate filled-form path
remain open. These original dimensions/lighting choices are reader policy.

Complete richer FreeText and other ledger subtype appearances;
add border effects, remaining callout combinations and required font/layout support.
Cover state dictionaries and rollover/down appearances where interaction uses
them. Model flags, geometry and hit testing consistently across render and host.

Acceptance: existing-AP and generated-AP versions have equivalent reviewed visual
and geometry results at rotations/zooms; NoZoom/NoRotate, visibility, optional
content and printing behave correctly. A media/3D poster appearance does not
count as interactive media/3D support (V03).

## V02 — Complete AcroForm workflows

Owner: `pagura-document`, `pagura-appearance`, `pagura::editing`, `web`;
depends on V01, R04 and E01 for repair/encrypted-save paths.

Add rich-text/Unicode/CID appearance generation, oblique/reflected/rotated text,
negative-size handling where valid, rounded/beveled/inset borders, complex Tx
updates, inline-image/XObject preservation, generated button states and nested
direct field identities. Complete field types/flags and reset/import/export
workflows identified by G00. Define password-field handling explicitly; the
current refusal is not silently relaxed. Share semantics between preview and save.

Acceptance: independent field values and appearance expectations survive filling,
preview, save/reopen and flattening in applicable readers. Untouched appearance
artwork survives; failures are atomic and preserve drafts. Browser tests cover
keyboard operation, multiline/comb/choice fields, radio groups, optional content,
rotation, page virtualization and repeated saves. Scripted events depend on V03.

## V03 — XFA, scripted actions and special interactive content

Owner: document/host/viewer boundaries; depends on G00/G01, V02, D01 and a new ADR.

Current policy forbids scripts during PDF interpretation. Propose a separate,
opt-in host execution interface with bounded memory/work, cancellation and
explicit event/data exchange; retain an inert engine. The ADR must reconcile
this with CONTRIBUTING/ADR 0005, safe Rust/dependency rules and reproducibility.
Do not mark the parity gap closed merely because execution is intentionally absent.

Deliver XFA packet/model parsing, static layout, dynamic layout/reflow, data binding,
validation and persistence; then the baseline-supported JavaScript/FormCalc event
and calculation subsets. Inventory document/page/field actions, reset/submit/import,
remote targets and media/3D separately. Implement each baseline-supported action
through explicit host capabilities; no document implicitly gains IO or submission.

Acceptance: compare field event order, calculations, validation, dynamic page
counts, layout and saved data against capable XFA/script builds. Infinite loops,
recursion, cancellation and denied capabilities have deterministic outcomes.
Unsupported script APIs remain visible. If an ADR retains an exclusion, record
an outstanding difference and use a narrower product claim, not “full parity.”

## V04 — Text, tagged structure and accessibility

Owner: `pagura-extract`, `pagura-text`, host and `web`; depends on G01/R04.

Complete custom structure namespaces/RoleMapNS, cross-stream/page replacements,
object references, tables/lists/figures and artifact semantics. Add bidirectional
and script-aware reconstruction for extraction/search/copy. For OCR or table
extraction confirmed in G00, define the optional provider/data/dependency contract
and add the corresponding PDF extraction workflow with independent text/table
oracles; ordinary text extraction must not silently claim OCR. Feed the existing
logical view into a semantic accessibility tree while retaining geometric search
and selection. Authoring/preserving structure during edits depends on E02.

Acceptance: known logical strings, reading order, languages, roles and source
quads pass independently of pixels. Mixed RTL/LTR, vertical text, ligatures,
ActualText, cross-page structures and untagged fallback are covered. Real browser
screen-reader/keyboard tests verify offscreen navigation and virtualized content;
PDF/UA validation remains a distinct C01 gate.

## V05 — Layers, printing and viewer integration

Owner: `pagura-interpret::resources::optional`, `pagura-host`, `web`; depends on
G01, R05, V01/V04.

Expose OCG configurations/order/radio groups/locks and user layer state; complete
View/Print/Export usage and relevant zoom/language conditions. Add print intent,
page/box selection, viewer/print preferences and an overprint control. Surface
attachment, signature and annotation tools from D02/S01/E02 as those APIs land.

Acceptance: independently expected layer membership and output differ correctly
between screen, print and export; UI state round-trips without cross-document
leaks. Print/export uses engine semantics, not a screenshot of mounted canvases.
Native/browser output uses identical settings and valid page geometry.

## D01 — Progressive input, cancellation and scheduling

Owner: `pagura-document`, `pagura-interpret`, `pagura-paint`, `pagura-render`,
host and `web`; depends on G01 and proposed ADR 0035.

Use the [existing detailed design](plans/async-pdf-loading-and-image-streaming.md):
P0–P3 define source/fault contracts, bounded windows, cache publication, ordinary
tail-first opening, linearization and requested-page dependencies; P5 adds a
persistent cursor to the existing band driver; P7–P9 integrate and validate jobs,
source supply and backpressure. Preserve complete-buffer convenience APIs and
atomic scene recording/source-order preflight. Supply unavailable ranges through
host capabilities; do not cache temporary absence as a terminal PDF error.

Between-band cancellation already works in library calls. Remaining work is
resuming without replay, exposing jobs/cancellation in the host, and bounding
latency in source/preparation/codec stages. Bounded native/WASM worker scheduling
is a subsequent D01 slice, outside the async plan's first release. Keep one
shared `PreparedScene`/caller-owned scratch model; parallelism cannot add a
different rendering route.

Acceptance: first-page display works before whole-file arrival on linearized
fixtures; non-linearized files request required ranges correctly. Short/overlapping
reads, cancellation, corrupt hints, changed source identity, missing ranges and
slow providers are covered. Full and ranged input produce identical results
within the same declared validation scope, as specified by ADR 0035.
Measure bytes fetched, time to first page, memory and cancellation latency under
predeclared budgets; incomplete work cannot produce a successful partial page.

## D02 — Document services and export

Owner: `pagura-document`, `pagura-image`, `pagura-codec`, facade/CLI/host; the D02a
read/extract/export APIs depend on G01 and D03 for encrypted attachments. D02b
modification APIs follow E01/E02; E02 consumes D02a, so there is no dependency cycle.

**File-view implementation started:** typed specifications, EmbeddedFiles name
trees, catalog/page/object AF arrays, EF streams and RF related files now have
bounded library/facade views. Fifty-one independently authored fixtures cover
byte/Unicode names, source identity, payload filtering, absent/null/empty cases,
malformed types, cycles, reported text/tree repairs, budgets and synthetic
attachment authorization. EF/RF context selects attachment crypt filters even
when the stream omits its optional Type; an explicit Identity filter retains
its meaning. Hosted library checks passed in
[run 40263961382](https://github.com/jonasgrosch/pagura/actions/runs/40263961382).
Host/viewer annotation selection, collection/encrypted-payload models, semantic validation,
streaming, writes and CLI/host AF/RF selection remain open; this does not complete D02.

**Metadata implementation started:** `8b328025a502cd9b7a3c5c61f2bac94aea12f5a9` adds typed Info strings and
trapping declarations, document language, and document/object metadata-stream
handles through the library/facade. Independent fixtures cover Unicode,
absence/null/empty distinctions, source identity, compressed packets, malformed
types, cycles, reported text repair, limits and inert XML bytes. The library
change passed hosted Linux/macOS/Windows checks in
[run 40262084291](https://github.com/jonasgrosch/pagura/actions/runs/40262084291). XMP/schema and date/language validation, reconciliation and writes remain open;
this does not complete D02. The host/viewer read slice is described below.

**CLI read/extract slice:** `a5149dd59de0d2b0cb3fc9894529ac7997571c1d` adds
versioned metadata/catalog attachment JSON and explicit-file packet/payload
extraction through the public facade. Unicode filenames and raw name/path bytes
remain separate; no document filename becomes an output path. Decoding and
authorization finish before creating a new destination; existing files are never
overwritten. [CLI documentation](CLI.md) defines the schemas and selection rules.
CLI checks passed on hosted Linux/macOS/Windows at `f1ab906ef` in
[run 40265512835](https://github.com/jonasgrosch/pagura/actions/runs/40265512835).

**Host/viewer read slice:** `ec6745e062c746dd94741dd826545c15a04e17cb` adds protocol 1.8
metadata records, catalog attachment listings and separate byte requests. All
responses use the reading-response budget; document view/stream limits bound
preparation separately. Listings never decode payloads. The viewer uses inert
text, exact declared sizes and explicit downloads with fixed names; stale
document/dialog requests cannot download. Native/WASI/client tests cover exact
independent fixture bytes, old versions, malformed requests, authorization and
limits; UI tests cover inert labels, failures and races. Hosted Linux/macOS/Windows
checks pass at `92eab7f34` in
[run 40269303206](https://github.com/jonasgrosch/pagura/actions/runs/40269303206).
Object-local metadata, CLI/host AF/RF selection, attachment-only viewer password
reauthentication, streaming, validation and writes remain open.

**Attachment parameter checks:** library/facade `EmbeddedFile::check_parameters`
and CLI `check-attachment` compare Size and CheckSum against completely decoded,
authorized bytes. Absent declarations, matches and mismatches are separate
observations; the checksum follows Table 45 and is not an authenticity result.
Seven independently generated compressed fixtures cover each combination.
Tests also cover empty files, encrypted attachment-only payloads, explicit
Identity, external-stream refusal and decode limits. Hosted Linux/macOS/Windows
checks pass at `e4e0e7471` in
[run 40272672956](https://github.com/jonasgrosch/pagura/actions/runs/40272672956).
The operation does not modify the source or create a payload file. Protocol 1.10
and explicit viewer Check controls now expose the same observations with exact
BigInt sizes. Tests cover old versions, canonical messages, response/decode caps,
selection/authentication, native/WASI/client parity and stale dialog callbacks.
These host/viewer checks pass at `64d6462b6` in
[run 40274019224](https://github.com/jonasgrosch/pagura/actions/runs/40274019224). Streaming and
semantic/date/MIME/profile validation remain open.

**Attachment annotation views:** `Page::attachment_annotations` discovers page
FileAttachment entries separately from catalog EmbeddedFiles and AF views. It
preserves original indexes, resolved annotation/FS/payload identities, normalized
geometry, Contents text and declared/default icon names. FS/Desc does not replace
annotation Contents. Forty independent fixtures cover page-only attachments,
reference/geometry indirection, external files, duplicates, malformed/late failures,
reported Unicode repair, budgets, compressed and encrypted payloads. Discovery
keeps payload decoding and authorization lazy; no file is activated. Independent
blank/red-rectangle oracles cover direct/indirect null Annots and indirect Rect
coordinates. The renderer now handles those valid forms. Typed-view fixtures distinguish malformed visual fields from file-specification
validation. The V01 icon increment now replaces the previous appearance refusal
for valid FileAttachment annotations; independent icon oracles are described above. Library/facade and independent render checks pass on hosted Linux/macOS/Windows at `5172155b3` in
[run 40277191350](https://github.com/jonasgrosch/pagura/actions/runs/40277191350). CLI attachment
commands now accept `--page PAGE` with distinct annotation JSON and exact original
Annots index/EF selection for extraction and checks. Tests cover selection without
catalog fallback, caller-chosen new files, password failures and no partial JSON.
These CLI checks pass on hosted Linux/macOS/Windows at `dc88606fb` in
[run 40277261503](https://github.com/jonasgrosch/pagura/actions/runs/40277261503).
Protocol 1.11 and client APIs now
list page attachments and retrieve/check an exact Annots slot and EF key, using
the shared file record and decode/check helpers. Tests cover version gates,
original slots, independent count/response/decode limits, authentication and
native/WASI/client equivalence. The host/browser suite passes at `e28dc0798` in
[run 40278830687](https://github.com/jonasgrosch/pagura/actions/runs/40278830687);
the Linux/macOS/Windows functional matrix also passes in
[run 40278830795](https://github.com/jonasgrosch/pagura/actions/runs/40278830795). The viewer now lists page attachments
separately, starts on the current page and offers explicit page selection,
downloads and parameter checks. Original slots/EF keys and fixed download names
are preserved. Annotation Contents and file descriptions remain separate inert
labels. Page changes invalidate old lists and decode callbacks; page-listing
errors preserve independent document controls. UI race/selection and the full
hosted functional matrix pass at `dde52cd7f` in
[run 40279713761](https://github.com/jonasgrosch/pagura/actions/runs/40279713761);
real-browser interaction validation remains open. Icon activation,
AF/RF selection and attachment-only password reauthentication remain open.

**PNG facade/CLI slice:** `0288c02118232228254c1a26af2856b443a24be4` exposes the
existing bounded PNG codec through `pagura::png`. Rendered page/tile Pixmaps are
consumed and converted to straight RGBA in place. CLI `render`/`convert` accepts
`--format png` and optional `--transparent`; default PPM behavior is preserved.
Independent Python decoding checks pixels, CRC/chunk framing and zlib completion;
API tests cover caller budgets and in-place conversion. Hosted Linux/macOS/Windows
checks pass at `2b314bc5c` in
[run 40271087997](https://github.com/jonasgrosch/pagura/actions/runs/40271087997). This is whole-buffer output; incremental PNG/DEFLATE, persistent
jobs, ancillary color metadata and backpressure remain open. Host/viewer export
is covered by the next slice.

**PNG host/viewer slice:** `335dbbd9c359b13d6f2ca30768bb0fd596907366` adds protocol
1.9 complete-page PNG bytes with explicit resolution/background. Surface budgets
are checked before recording; PNG stream and response frame caps are independent
of reading-response limits. More → Export page PNG includes copied current form
values through a temporary saved handle, released on success/failure/staleness.
It does not adopt the snapshot or erase drafts. Native/WASI/client byte checks,
independent Python pixel decoding, wire/version/budget tests and viewer
option/snapshot/race/cleanup tests pass in hosted Linux/macOS/Windows checks at
`f4ee9bcef` in [run 40272959779](https://github.com/jonasgrosch/pagura/actions/runs/40272959779).
Real-browser interaction verification remains pending. Export uses PDF page orientation. Streaming, active
cancellation, backpressure and ancillary color metadata remain open.

Add typed metadata/XMP, embedded/associated files, file specifications, thumbnails,
portfolio/collection views and baseline-supported document properties. Expose
lossless original-image extraction where valid and decoded-image export otherwise,
including masks, colour interpretation and provenance. Connect existing PNG/zlib
encoders to public conversion APIs/CLI/host. The existing band callback and CLI
PPM streaming need no replacement; add incremental PNG/DEFLATE, persistent output
jobs and async backpressure using P4–P9 of the linked D01/D02a design. P4 owns
assets/occurrences, P5 the shared cursor and P6 the encoder; P7–P9 integrate them.
Probe PDF-to-SVG/vector/raster and text export formats exposed by the baselines;
add confirmed PDF-origin export obligations to this package rather than equating
PNG alone with export parity.

Acceptance: synthetic attachments/image bytes and hashes match independent
expectations; filenames/URLs remain data and extraction never executes content.
Exports round-trip through independent decoders, distinguish original vs rendered
assets and respect permissions/budgets. A failing sink reports failure without
claiming a complete output; profile metadata is not a validation result.

## D03 — Encryption and authorization coverage

Owner: `pagura-crypto`, `pagura-pki`, `pagura-document`, host; depends on G00/G01 and
an ADR for private-key recipient operations.

Implement public-key security through a host key-operation provider; inventory
baseline-supported newer security extensions rather than assuming support.
Complete encrypted revision access, attachment authorization, UI legacy password
encoding and permission reporting. Keep permission policy distinct from password
or key authentication; use synthetic credentials only in fixtures.

Acceptance: independent encrypted documents cover recipient selection, wrong keys,
password encodings, crypt filters, metadata, embedded files and historical views.
No embedded credential grants authority. Encryption-on-write is E01 and must
round-trip independently before encrypted edits are advertised.

## E01 — Full rewrite and encryption on write

Owner: `pagura-write`, `pagura::editing`, document/crypto interfaces; depends on G01,
D03 for public-key output and an editing/writer ADR.

Add a reachable-object graph writer, explicit deletion/free-list policy,
resource/reference remapping, object streams, compression and PDF 2.0 header/ID
handling. Define repair-mode source rewrite and unknown-object preservation.
Implement standard encryption output, then any required public-key output, with
host-supplied key material and signature-aware full-rewrite refusal/policy.

Acceptance: independent qpdf checks plus applicable renderer reopen verify page
counts, semantic content and appearances; repaired sources do not preserve broken
structure. No-op incremental bytes remain exact. Rewrite reports signature
invalidation, never preserves a misleading validity claim. Encrypted output
passes independent readers with correct/incorrect credentials and all filters.

## E02 — General editing and form/annotation authoring

Owner: `pagura::editing`, `pagura-document`, `pagura-content`, `pagura-appearance`,
text and `web`; depends on E01,
V01/V02/V04 and D02a's read/model APIs for affected services.

Add typed page insertion/deletion/reorder/merge/split/crop/rotate; annotation and
form create/update/delete/flatten; content text/image/path operations; font
embedding/subsetting where required; navigation, tags, metadata and attachment
updates. Implement transaction history/undo/redo without mutable source caches.
Use shared-resource copy-on-write and explicit text/layout semantics.

Acceptance: edit/save/reopen checks include resource/name collisions, shared
XObjects/fonts, tagged references, cross-document imports and repeated undo/redo.
Unedited pages and resources retain their required semantics. Raster output is
not used as the only editable representation; authoring UI tests cover the full
preview/commit/reopen cycle and atomic refusal.

## E03 — Sanitizing redaction

Owner: editing/writer plus extraction/document services; depends on E01/E02/D02/V04.

Remove selected text/image/vector content and associated hidden representations,
including clipped content, forms/XObjects, layers, ActualText, tags, metadata,
attachments and obsolete revisions as required by the declared redaction policy.
Always produce a sanitizing rewrite; an incremental black rectangle is not redaction.

Acceptance: independently seeded canaries disappear from decompressed reachable
objects, old revisions, text extraction, metadata/attachments and rendered output
in applicable readers. Check shared resources and hidden/alternate content.
Unresolvable references or uncertain removal fail atomically; the output cannot
be called sanitized when any retained representation still exposes the target.

## S01 — End-to-end signature validation

Owner: `pagura-pki`, `pagura-crypto`, PDF signature facade, host/viewer; depends on
G01, D03 for encrypted history and V04 for accessible results.

Connect existing path/CRL validation to CMS signers using a real bounded crypto
verifier, host anchors, evaluation time and policy. Add offline OCSP, issuer-name
matching, TSA authorization/clock policy, DSS/VRI evidence and required PAdES/LTV
semantics. Extend structural analysis for outstanding transforms and permitted
DSS/timestamp changes; connect it to save enforcement. Preserve separate results.

Acceptance: CAVP/Wycheproof/PKITS and independently signed PDFs cover trusted,
untrusted, revoked, stale/missing evidence, altered bytes, multiple revisions,
hidden historical signatures, timestamps and allowed/forbidden edits. Existing
PKITS callback tests are supplemented by real cryptographic integration tests.
No trust/evidence/time input means unknown or untrusted, never valid. The viewer
shows results outside PDF artwork; write policy consumes the same analysis.

## S02 — Signature creation

Owner: writer, signature facade and host; depends on E01/S01/D03 and an ADR
amending ADR 0015's verification-only scope.

Define a host-owned signing provider rather than adding secret-key operations to
the current verification primitives. Add ByteRange/Contents reservation, detached
CMS/CAdES, timestamp/evidence insertion and signature widgets; retain explicit
key/certificate selection and authorization at the host boundary.

Acceptance: independent validators verify produced signatures and timestamp
imprints; repeated signing preserves earlier covered bytes. Failed/oversized
provider output is atomic, and injected wrong signatures are rejected. Key
material never enters document content or logs. Existing verification-only scope
is an open parity decision until the new contract is accepted and implemented.

## C01 — Version, profile and extension coverage

Owner: document/schema and validation APIs, fixtures; depends on G00 and applicable
R/V/D/E/S packages.

Build a clause/feature inventory for PDF 1.x, PDF 2.0 and baseline-supported
extensions. Separate reading/rendering from validation and conforming creation.
Add explicit profile validators for PDF/A-1/2/3/4 and PDF/UA-1/2; cover PDF/X
print/profile requirements and any baseline-supported PDF/E/PDF/VT or associated
file workflows admitted by G00. Define each exact conformance level before coding.
Profile-preserving edits and conversions depend on E01/E02/R05/V04/D02.

Acceptance: independent positive/negative profile suites, including veraPDF where
applicable, agree on reviewed requirements. Validate font embedding, metadata,
colour/output intents, forbidden features, structure/accessibility and attachments.
A metadata label, successfully opened version header or passing renderer is never
reported as profile conformance. Every required profile/level has its own gate.

## C02 — Final compatibility, robustness and performance evidence

Owner: test/benchmark tooling and affected components; depends on all required rows.

Run the full pinned corpus with all pages, refresh scoreboards at the release
candidate and investigate every relevant difference. Expand the real-world corpus
under ADR 0007 after its stated trigger, preserving input hashes. Run sustained
coverage-guided fuzzing for the new paths and replay all findings. Finish
cold-open, cold/warm render, tile/zoom, text-search, no-op-save and edit-save
adapters and bind every workload in `benchmarks/release-matrix.json`.

Acceptance: zero unknown/unassigned required capability cells, zero unexplained
correctness regressions, no crashes and no unadjudicated timeouts. All independent
oracles, native/WASI parity, feature configurations, browser workflows and
save/redaction/signature tests pass. Run ADR 0002's controlled comparisons on the
required hardware/DPI/concurrency matrix: randomized pairs, at least 100 samples,
five warmups, one-sided 95% upper bounds at most 0.80 for median and p95, with
no peak-memory regression. Missing cells block release; functional parity and
performance superiority remain separate claims.

## First implementation changes

Start with these reviewable changes; each includes fixtures and its ledger/status
update. Reorder later display work using G01's measured failure frequencies.

1. G00 is complete for the reviewed source scope. Keep the checked inventory
   current as packages land, and reopen it when new call sites or API families
   are discovered. Source classifications do not establish runtime parity.
2. G01 adapter controls and current corpus baseline. Publish reproducible summaries
   and the ordered queue of valid missing features, not a single success percentage.
3. R02 embedded ICC/sYCC interpretation and colour precedence, using existing
   colour transforms and independently calculated reference samples.
4. R02 Indexed opacity/channel mapping and remaining image boundaries; retain
   existing straight/premultiplied alpha and add group/mask interaction oracles.
5. R04 vertical CJK substitution, with exact glyph-choice/advance/placement cases
   for the supported collections and no-provider builds.
6. V01/V02 separate changes for missing annotation appearances, then generated
   button states/direct field identities and form save/reopen coverage.
7. D01/D02a P0 contract review of proposed ADR 0035, then P1–P3's minimal
   range-backed first-page path. Reuse the existing band API; P5/P6 cursor and
   incremental PNG work can proceed independently on complete inputs.
8. S01 PDF signer-to-path/CRL integration with real cryptography, explicit host
   anchors/time and tests proving `NotCheckedYet` changes only when checked.

Before dependent implementation, prepare the V03, D03 and S02 policy ADRs and
E01 rewrite contract. Planning these decisions does not silently change current
product policy or declare the corresponding gap closed.

## Definition of done for every implementation change

- Record the baseline build/API and normative or interoperability requirement.
- Add independent valid, malformed and resource-limit fixtures, including a
  meaningful interaction with an already supported feature.
- Implement within safe Rust, clean-room and bounded-resource rules; maintain
  stable diagnostics and distinguish strict rejection from reported repair.
- Exercise the intended public surface, not only a private helper. Run
  `uv run tools/check.py` and targeted cross-engine/browser/profile checks in
  the permitted validation environment. Preserve ADR 0031's hosted-validation
  workflow when implementation shares the machine with performance lanes.
- Update requirement status, STATUS.md and capability evidence together. Record
  source/binary/corpus identity; retain unknowns and incompatible baselines.
- For rendering changes, enforce ADR 0004. For performance complexity, provide
  the measured justification required by ARCHITECTURE.md. No unmeasured speed
  claim or weakened correctness threshold closes a feature row.
