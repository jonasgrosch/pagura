# Plan to close competitive PDF feature gaps

Status: implementation in progress against `render/one-pass` at
`73e1ab018d9c23f24c374de1f3a4441b119a5365` (PR #118), reconciled 2026-10-26.
G00's reviewed source inventory is complete. G01 and the feature packages below
remain open until their acceptance gates pass. The checked
[capability inventory](CAPABILITIES.md) identifies support on this branch;
a feature in another open PR is not support on this branch.

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
| [Accepted ADR 0036](adr/0036-progressive-sources-and-streaming-jobs.md) | Accepted source ownership, validation, job-budget and extraction-identity contracts; implementation and validation remain group 11 work. |

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

## Review groups and PR budget

Use **18 feature groups, with a hard ceiling of 20 PRs for the remaining plan**.
The two spare slots permit a whole feature to split when its size or dependency
makes review materially clearer. They are not for probes, adapter-only changes,
CI fixes, documentation follow-ups or individual annotation subtypes. Previously
merged inventory work is retained; superseded open PRs are closed after their
changes and fixes are accounted for in the replacement group.

| Group | Complete review boundary | Packages | Dependency |
|---|---|---|---|
| 01 | Capability comparison and final release evidence | G00, G01, C02 | Probes travel with each feature; final evidence follows all groups |
| 02 | Document structure, reported recovery and explicit external data | R01 | 01 |
| 03 | Image codec variants and PDF image integration | R02, R03 | 01; existing decoder/colour interfaces |
| 04 | Font programs, substitution and text binding | R04 | 01 |
| 05 | Colour, transparency, overprint and graphics correctness | R05, R06 | 01, 03, 04 as needed |
| 06 | Annotation display and shared appearance generation | V01; shared V02 geometry | 04 for richer font coverage |
| 07 | Complete AcroForm filling, preview, persistence and data exchange | V02 | 06; 14 for encrypted/repaired saves |
| 08 | Explicit host actions, XFA and bounded script execution | V03 | 07, 11; scope ADR |
| 09 | Logical text, search and accessible structure | V04 | 04 |
| 10 | Layer state, print intent and viewer output | V05 | 05, 06, 09 |
| 11 | Progressive source input, resumable jobs and streamed encoding | D01; D02a P0–P9 | 02; ADR 0036 |
| 12 | Metadata, files, collections, image extraction and complete export APIs | D02a | 03 where image interpretation is needed |
| 13 | Encryption access and end-to-end signature verification | D03, S01 | 02; explicit host trust/key contracts |
| 14 | Full rewrite and encrypted output | E01 | 13; writer contract |
| 15 | Page/content/form/annotation authoring and document-service writes | E02, D02b | 06, 07, 09, 12, 14 |
| 16 | Sanitizing redaction | E03 | 09, 12, 14, 15 |
| 17 | Signature creation and timestamp/evidence insertion | S02 | 13, 14; signing-provider ADR |
| 18 | Version, extension and exact profile conformance | C01 | Relevant feature groups |

The current review consolidation retains #213 for document services and complete
PNG export, #256 for appearance generation, and the #182/#187 image work as one
image-integration group. The independent source branches remain recoverable.
A retained PR is draft until its advertised boundary, public surfaces and hosted
checks are complete. Remaining work stays assigned in this plan, even if a group
cannot yet claim the entire package's acceptance gate.

Rebuild each review branch from the current `render/one-pass`, carrying only its
feature and required shared changes. A dependent branch may be developed privately,
but open its PR after the prerequisite lands so the review does not repeat the
prerequisite diff. All PR targets remain `render/one-pass`. Group-specific probes,
fixtures, requirements and capability updates belong in the same PR as the feature.

## Architecture and size controls

| Owner | One implementation to extend |
|---|---|
| `pagura-document` | Immutable source/revision identity, bounded typed views and traversal; host-supplied input only |
| `pagura-appearance` | Shared font encoding/layout, border geometry and appearance programs for annotations and forms |
| `pagura-interpret` | PDF resource resolution and the single device dispatch; no second image, text or annotation interpreter |
| `pagura-color` / `pagura-image` | Object-free transforms and codec/layout types; PDF policy stays at the interpreter boundary |
| `pagura-paint` / `pagura-raster` | Source-ordered preparation and one exact band executor; export and preview consume the same output |
| `pagura::editing` / `pagura-write` | Immutable transactions and one bounded serializer; typed operations reuse these layers |
| CLI / host / viewer | Argument/wire conversion and user state; no duplicate PDF traversal or appearance renderer |

Before adding a module, identify the existing owner and the code it replaces or
extends. Share a helper when it represents an invariant used by multiple callers;
do not add generic registries, forwarding layers or parallel models for imagined
future use. Remove the superseded path in the same change. Generated test data
must remain separate from product code and independent of the implementation.

Each PR description records its public behavior, dependency, product-code delta,
removed paths and meaningful tests. Review source and authored generators before
generated binaries. Large fixture counts do not justify a large product diff;
reviewers must still be able to follow one feature through its existing owners.
Preserve bounded work/storage, safe Rust, workspace-only product dependencies and
the exact-output contracts. Performance-only complexity needs measured evidence.

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

The existing harness records engine/build identity, configuration, fixture hashes
and separate unsupported/incorrect/limit/timeout/adapter-failure outcomes. Service
and rendering probes expand with the feature groups that introduce their fixtures.
The current appearance work contains 180 independent render controls; the service
work contains 21 controls across the six baselines and Pagura. Exact run identities
and observations live in [the probe record](../benchmarks/probes/README.md).
Fixture passes never promote an entire family. All 258 family comparison cells
remain unprobed until the wider operation/configuration obligations are established.
Hayro/CoreGraphics text adapter gaps and artwork/font/sampling disagreements remain
explicit work, not evidence that the baseline lacks the feature.

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

The one-pass base already integrates embedded ICC, eight-bit sYCC and direct
Indexed opacity. The image group consolidates broader sYCC precision and matched
controls with remaining colour selection, Lab input, palettes and inline framing.
Only the branch inventory and source-bound hosted results establish what has landed.

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
amendments before implementation. Keep codec-specific commits inside the image review group; split a PR only under the review budget above.

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

The consolidated appearance group covers standard icons, geometric markup,
Line/PolyLine endings and leaders, captions, text markup, Caret, stamps, FreeText
and shared rectangular border styles. It reuses font encoding/layout and the
same border geometry for filled forms. Independent authored pixels, malformed
input/budget controls, stored-appearance precedence, partial tiles and native/WASI
client checks accompany the feature. Exact supported subvariants are in the
branch's capability inventory; this list does not imply the entire V01 gate passes.

Rich text, cloudy effects, wider border/callout combinations, nonempty widget
fallback, interaction-selected R/D states and remaining font/layout cases stay
assigned here or to V02. New implementations join this group instead of opening
another PR for each subtype, ending, border or probe increment.

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
negative-size handling where valid, remaining rounded stroke/dash evidence, complex Tx
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
host and `web`; depends on G01 and accepted ADR 0036.

Use the [existing detailed design](plans/async-pdf-loading-and-image-streaming.md):
P0–P3 define source/fault contracts, bounded windows, cache publication, ordinary
tail-first opening, linearization and requested-page dependencies; P5 adds a
persistent cursor to the existing band driver; P7–P9 integrate and validate jobs,
source supply and backpressure. Preserve complete-buffer convenience APIs and
atomic scene recording/source-order preflight. Supply unavailable ranges through
host capabilities; do not cache temporary absence as a terminal PDF error.

Owned render sessions and incremental PNG encoding now preserve cursors and
bounded output leases. Remaining work includes source/document integration,
host jobs and cancellation, and bounded latency in loading/preparation/codec
stages. Bounded native/WASM worker scheduling is required within group 11,
including priorities, aggregate queue/memory admission, starvation controls and
shutdown. Source/output waits release CPU admission. Keep one shared
`PreparedScene` and private worker scratch model; parallelism cannot add a
different rendering route. The complete group remains open until these
scheduling and source/transport gates pass.

Acceptance: first-page display works before whole-file arrival on linearized
fixtures; non-linearized files request required ranges correctly. Short/overlapping
reads, cancellation, corrupt hints, changed source identity, missing ranges and
slow providers are covered. Full and ranged input produce identical results
within the same declared validation scope, as specified by ADR 0036.
Measure bytes fetched, time to first page, memory and cancellation latency under
predeclared budgets; incomplete work cannot produce a successful partial page.

## D02 — Document services and export

Owner: `pagura-document`, `pagura-image`, `pagura-codec`, facade/CLI/host; the D02a
read/extract/export APIs depend on G01 and D03 for encrypted attachments. D02b
modification APIs follow E01/E02; E02 consumes D02a, so there is no dependency cycle.

The document-service group consolidates typed Info/XMP views; EmbeddedFiles,
catalog/page/object AF, EF/RF and page attachment discovery; explicit payload
selection and declared size/checksum checks; collection/encrypted-payload
metadata; stored thumbnail and image-asset extraction; and complete page PNG
export. Library, CLI, host and viewer are one review boundary. Packet/filename/URL
bytes remain inert and extraction writes only to an explicit caller destination.

Keep exact stored assets, unwrapped encoded images, interpreted image occurrences,
normalized image output and page crops distinct. Share image description, colour
and mask resolution with the interpreter. P4 of the linked D01/D02a design owns
resource-context-aware image occurrences; P5/P6 extend the existing band driver
and PNG/DEFLATE encoder. P7–P9 own jobs, async integration and backpressure. These
remain in group 11, rather than a second extraction/rendering implementation.

Validate association/MIME/date and metadata declarations where the standard
requires them; a metadata/profile label alone is not conformance. Add confirmed
PDF-origin SVG/vector/raster/text export obligations found by G01. Complete PNG
output is not a claim of streaming or all-format export parity. Metadata/files,
collections and associated-file modification belong to D02b in group 15.

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

## Current implementation order

1. Finish the three consolidated image, appearance and document-service groups,
   including simplification, public integration and their hosted failures.
2. Keep G00 evidence accurate and expand G01 probes in those same groups.
3. Prepare font, colour/graphics and AcroForm groups against their exact dependencies.
4. Prepare source/jobs, rewrite, host-action and signing contracts before changing
   those boundaries; implement independent groups in parallel.
5. Reserve final compatibility, robustness and performance evidence for the actual
   integrated release candidate. A draft or a green subset cannot close the plan.

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
