# Capability and evidence status

Pagura has substantial rendering, reading, form-filling and incremental-writing
support. It is **not a complete PDF implementation**, and feature parity with
PDFium, MuPDF, pdf.js, Poppler, hayro and CoreGraphics has not been established.
Work spans M1–M7; none of those milestones is declared complete by this page.

This inventory was reconciled against `render/one-pass` at
`847fb26f0616e8b70bec76883880c78cbc47915d`, the embedded-colour follow-up to the fetched
one-pass head `7c03b656d1e1093c6951e80ec2fc189b5b1bf90a` (PR #118).
It describes that branch, including refactoring not yet merged into `main`.
**Implemented** below means an implementation and associated tests exist, within the stated
limits. It does not mean those tests or all corpora were rerun for this audit.
Library support, PDF integration and viewer support are listed separately where
they differ. The [competitive parity plan](COMPETITIVE_PARITY.md) owns the
remaining work, dependencies and acceptance criteria.

G00 source inventory is complete for this revision in the [checked inventory](CAPABILITIES.md):
43 capability families, all 90 literal tags reviewed, and all 335 requirement IDs assigned.
It includes API families without tags and separates library, CLI, host and viewer support.
All 258 competitor family cells remain unprobed; G01 owns executable probe evidence.
The source check detects stale evidence and missing ownership, not runtime parity.

## Capability matrix

JBIG2 also reports `jbig2.unused-aggregation-table` for the unused Huffman
aggregation selector when aggregation is disabled (T.88 7.4.2.1.1). Its table
remains parsed, validated and budgeted; genuine aggregation, arithmetic coding,
other flag errors and malformed references retain their checks. Only the first
occurrence in decoding order is retained, including correct Globals identity.
This narrow policy does not allow file envelopes, terminal page/file segments,
missing arithmetic markers or page-information reordering. New literal-matrix
controls cover valid twins, used aggregation and subsequent faults; broader
codec parity remains unestablished.

JBIG2 viewing reports `jbig2.page-capabilities` when complete regions contradict
the page's auxiliary/refinement capability bits (T.88 7.4.8.5). Strict mode
rejects the same input. This repair changes neither region validation nor
resource limits; it does not establish broader JBIG2 compatibility.
Refinement line prediction reuses the template state with only its reference
centre set (T.88 6.3.5.6, Figures 14–15). The decoder and independent encoder
now map that symbolic state through their own template bit order; geometric
expected bitmaps remain unchanged when the TPGR streams are re-encoded.

Raw JPEG 2000 codestreams in PDF images require the reported
`jpx.raw-codestream` viewing repair, an explicit PDF colour space and no
embedded-opacity request. Strict mode requires full JPX framing (7.4.9).
This does not infer missing colour/channel metadata or extend codec coverage.

| Area | Implemented | Remaining boundary |
|---|---|---|
| Rendering architecture | One interpreter with negotiated device needs; separate text device; appearance generation in `pagura-appearance`; cached resolution-independent scenes; source-order preparation, per-buffer N16/F32/F64 storage and one band executor; exact rendering only | Additional output/hit-testing devices, persistent resumable jobs and broader scheduling. `Accuracy::Competitive` and the former rendering pipelines have been removed. Architecture alone does not establish complete PDF feature coverage. |
| Core and document structure | Bounded syntax, generation-aware objects, classic/xref streams, object streams, hybrid files, incremental revisions, lazy object caches, page trees and reported reconstruction/repair | Broader malformed-file compatibility needs current corpus evidence. Opening requires complete bytes; linearization hints and range loading are not implemented. External-file streams remain unsupported. |
| Filters | ASCIIHex, ASCII85, RunLength, LZW, Flate, PNG/TIFF predictors and bounded decoding; explicit raw-DEFLATE entry point and reported compatibility repairs | Image filters dispatch to `pagura-image`; their tags in the generic filter crate do not mean the engine lacks those codecs. |
| Password encryption | Standard handler R2–R6, RC4/AES, crypt filters, permissions consistency checks and Unicode 3.2 SASLprep for R6; CLI legacy PDFDocEncoding password conversion | Adobe public-key security handlers; encrypted historical-revision access; encryption on write. The browser password path does not provide the CLI's legacy text conversion. |
| Font programs | TrueType/OpenType glyf; cmap 0/2/4/6/10/12/13 and post 1/2/3; CFF Expert/ExpertSubset charsets, FDSelect 0/3/4; CFF2 default-instance charstring and Private DICT blends; static Type 1/PFA/PFB programs, with reported retention of unused sparse Subrs slots; bounded indexed/named TTC v1/v2 faces with PDF selected-family validation | Nondefault variations, CFF2 Font DICT matrices, cmap 8, other post formats, unknown collection versions and unresolved/ambiguous face names, general PostScript, hybrid/synthetic Type 1, synthetic CFF, CFF CharstringType 1, standalone CIDFont programs and nonhorizontal Type 1 `sbw`. Hints are not executed. Primary-text verification remains incomplete for some implemented formats. See the [font inventory](../crates/pagura-font/README.md). |
| PDF text/font binding | Simple, Type 3 and CID fonts; embedded and predefined CMaps, ToUnicode, widths and vertical metrics; bundled standard-14 substitutes and opt-in CJK substitutes (`cjk-fonts`); shared document font resources; reported PDF collection-name policy and intact CIDFontType0 program-binding repairs (Strict rejects) | Vertical substituted CID fonts, unknown symbolic encodings/collections and executable CMap programs. Embedded vertical fonts are supported. Substitution coverage depends on the provider and build features. Contradictory standalone sfnt/font-family validation remains a separate gap. |
| Image codecs | JPEG baseline/extended/progressive, Huffman and arithmetic, 8/12-bit, CMYK/YCCK; CCITT G3/G4; JBIG2 generic/refinement/symbol/text/halftone/Huffman regions; JPEG 2000 Part 1 with progression, precincts, ROI and partial-quality reconstruction | JPEG lossless/hierarchical modes, CCITT extension mode, JBIG2 extended templates/necessary extensions, and JPX profiles/layouts outside the implemented subset. Codec success does not establish complete PDF image rendering. |
| PDF image integration | Inline images and XObjects, 1–16-bit samples, Decode arrays, masks, separate soft masks and interpolation/minification; JPX straight/premultiplied embedded alpha is decoded and composited, with Gray/RGB/CMYK embedded spaces, bounded embedded ICC forward transforms, eight-bit unsigned sYCC, direct Indexed index/opacity channels, and mixed-precision/reference-grid adaptation | Indexed JPX opacity with container palettes, other/mixed sYCC precisions, Lab-input ICC, multi-profile precedence and unsupported selected-transform fallback remain open. Gray/RGB embedded ICC and eight-bit sYCC have independent pixel fixtures; broader interactions still need evidence. See [image description](../crates/pagura-interpret/src/machine/image/describe.rs) and [painting](../crates/pagura-paint/src/source/image.rs). |
| Paths and shading | Fills/strokes, dashes, clipping, forms, tiling patterns, mesh shadings, functions, crop/rotation/UserUnit and tiled output | Pattern-coloured stencil cells clip geometry jointly; magnified Interpolate samples bilinear opacity once on complete pattern alpha and shape, while minification keeps exact cells; ordinary nested tiling fills/strokes and glyphs propagate inherited geometry through every pattern level. Multiple true-quadratic glyph ancestors and curved leaves use joint original-control coverage intervals, certified to one byte before composition; canonical cubic glyph semantics and existing resource limits remain authoritative. An unselected Pattern colour space remains an explicit refusal. Nested image stencils and transparency groups preserve joint clipping. Explicit refusals remain for singular transforms and pattern-placement/state cases; classify each as valid unsupported input, invalid input or a resource/numerical limit. Difficult mesh/geometry cases need independent accuracy evidence. |
| Colour and transparency | Device/Default/Indexed, ICC forward matrix/TRC and LUT processing, calibrated colours, Lab, Separation/DeviceN; Gray/RGB/CMYK transparency groups, standard blend modes, knockout and soft masks | ICC destination/inverse transforms and calibrated group destinations; nonzero calibrated black points/black-point compensation; fractional, pattern and per-colourant transfer-region selection. Supported ICC group destinations currently use their resolved device base. Lab is prohibited as a blending destination, not a feature to implement. |
| Overprint | CMYK process and spot display simulation, including group interactions, with `Auto`/`Always`/`Never` library and CLI options; catalog/page PDF/X intent detection | Destination ICC output transforms and transfer/overprint combinations; viewer control. Spot display uses alternate-based ink equivalents with a 32-plate bound, not physical ink prediction. PDF/X detection is not conformance validation. |
| Annotation display | Stored normal appearances and states; screen visibility, NoZoom/NoRotate and FixedPrint placement; generated geometric markup, FreeText, icons, borders and source text/choice/button widgets through the shared appearance generator. Valid indirect Rect coordinates resolve normally; missing inert Popup geometry is a reported Repair-only skip. | Reader-chosen icons and synthesized layout are policy rather than universal reference pixels. Nonlinear generated path strokes and dashed rounded strokes remain explicit refusals pending separate accuracy work; unsupported text encodings/transforms and malformed geometry remain errors. Playing media, applying redactions, general annotation authoring, and persistence of new generated button/rich-text forms remain separate gaps. New display coverage requires exact-head qualification. |
| AcroForms | Field tree/types/inheritance/widgets; text, checkbox, radio and choice filling; appearance generation; filled-page rendering; incremental saved values and appearances; browser controls, preview, discard and download | XFA, rich text, generated button states, nested direct field/widget identities, password-field persistence, complex appearance updates, rotated widgets and encodings outside the reversible simple horizontal model. See [editing](EDITING.md) for library and browser boundaries. |
| Navigation and actions | Name/number trees, destinations, outlines, links and page labels; inert action data; browser internal destinations and controlled URI activation | No JavaScript calculations/events or automatic action execution. General external-file, launch, submission and media actions need explicit host contracts. |
| Text extraction and accessibility | Positioned glyphs, words/lines/reading order, ActualText/Alt/ReversedChars, Unicode search and quads; separate tagged logical view; viewer search/copy/selection and screen-reader-readable DOM text | Tagged logical structure is not connected to the viewer accessibility tree. Custom namespaces/RoleMapNS and some cross-stream/cross-page replacements are unsupported; bidi/script-specific reconstruction remains incomplete. The DOM text layer alone is not PDF/UA support. |
| PDF signature structure | Current and discoverable historical signatures, source-bound ByteRange/revisions, DocMDP/FieldMDP structural modification analysis, per-signature errors and explicit inconclusive results | Inaccessible encrypted/repaired history, unsupported roles/transforms/template operations and DSS/timestamp exceptions remain inconclusive. Structural permission analysis does not establish cryptographic validity or enforce save policy. |
| CMS integrity | Detached CMS/CAdES and RFC3161 integrity, SHA-1 (reported weak), SHA-2, RSA v1.5/PSS and ECDSA P-256/P-384/P-521; signed attributes, ESS binding and timestamp imprints | PDF-facing signer trust, validation time and revocation still report `NotCheckedYet`; complete PAdES policy/LTV and legacy encodings remain incomplete. Issuer matching currently requires identical DER names. |
| PKI library | Host-anchored certificate path construction/validation, policy/name constraints, offline complete/indirect/delta CRL checking and bounded path search | OCSP, remaining name-normalization cases and application-specific timestamp/CMS policy; connecting these APIs to PDF signatures and the viewer. CRL/path support exists and must not be described as wholly absent. See [PKI scope](../crates/pagura-pki/README.md). |
| Writing and editing | Deterministic new PDF 1.7 files; immutable transactions; classic/xref-stream append revisions; saved forms; no-op byte preservation and rollback | Full rewrite, freed objects, object-stream output, PDF 2.0 new-file header/ID generation, encryption-on-write, sanitizing redaction, undo/redo and general page-content/page-tree editing APIs. Modified saves require strict, unencrypted input; signed input defaults to refusal. |
| Image output and cancellation | Public `RenderContext::render_bands` lends final premultiplied RGBA bands with a fallible consumer and between-band stop callback after preflight; the CLI streams PPM with bounded output storage. Library zlib compression and whole-buffer PNG encoding for eight-bit Gray/RGB/straight RGBA exist | Calls restart from the region's first row; persistent pause/resume, incremental PNG/DEFLATE, facade/CLI PNG export, async backpressure and public embedded-image extraction remain. Existing streamed PPM does not imply streamed PNG or progressive input. |
| Browser and WASM host | Native/WASI session protocol, viewport generations, bounded render `Store` for scenes/images/fonts/glyph setups, continuous scrolling/zoom, password prompt, text/navigation and form editing | Range input and resumable host jobs, active-request cancellation, parallel scheduling, layer/overprint/print controls and broader browser/device/accessibility validation. Library band-stop support is not wired into active host requests; generation cancellation discards stale queued work. |
| Profiles and document services | PDF 1.x/2.x headers and many shared features; low-level access to document objects; PDF/X declarations influence overprint policy | Complete PDF 2.0 feature coverage; PDF/A and PDF/UA validation; PDF/X conformance; typed attachment/associated-file, portfolio, metadata and richer document-service APIs. Rendering a profile-labelled file is separate from validating or authoring that profile. |
| Safety and release evidence | Safe Rust and workspace-only product dependencies, bounded APIs, deterministic replay, fuzz drivers, generated independent fixtures, conversion/regression harnesses | Long fuzz campaigns, host/process containment and cancellation evidence, refreshed compatibility reports, non-render operation adapters and the complete controlled performance gate. |

## Evidence and accuracy

Source entry points and existing tests supporting this reconciliation include:

- [Font program inventory](../crates/pagura-font/README.md),
  [font vectors](../crates/pagura-font/tests/vectors/static_fonts.rs) and
  [font requirements](../references/requirements/font.json).
- [Group colour-space resolution](../crates/pagura-interpret/src/resources/group.rs),
  [transparency requirements](../references/requirements/transparency.json),
  [overprint requirements](../references/requirements/overprint.json),
  [precision planning](../crates/pagura-paint/src/prepare/storage.rs) and
  [format tests](../crates/pagura-paint/src/prepare/tests/formats.rs).
- [Editing API/tests](../crates/pagura/tests/api/editing.rs),
  [form appearance implementation](../crates/pagura-appearance/src/form.rs)
  and [browser reading behaviour](../web/README.md#reading-search-and-navigation).
- [Signature discovery/change analysis](../crates/pagura-document/src/signatures.rs),
  [PDF-facing result types](../crates/pagura-document/src/signatures/model.rs),
  [CMS verifier](../crates/pagura/src/signatures.rs) and
  [offline CRL validation](../crates/pagura-pki/src/validation/revocation/mod.rs).
- [JPX alpha adaptation](../crates/pagura-image/src/jpx_image.rs),
  [alpha sampling](../crates/pagura-paint/src/source/image/texels.rs) and
  [paint fixtures](../corpus/manifests/paint.json), including `image-jpx-alpha-1/2`, plus the new [embedded colour fixtures](../corpus/manifests/paint_jpx.json).
- [PNG encoder](../crates/pagura-codec/src/png.rs) and
  [zlib compressor](../crates/pagura-codec/src/flate/encode.rs).
- [Public band/stop API](../crates/pagura-render/src/context.rs),
  [shared executor](../crates/pagura-paint/src/execute.rs),
  [streaming tests](../crates/pagura-paint/src/prepare/tests/streaming.rs) and
  [CLI PPM output](../crates/pagura-cli/src/commands.rs).

The existing detailed [recovery and information-diagnostic notes](RECOVERY.md)
are retained separately from this capability summary.

Rendering is exact throughout under ADR 0004's one-level error contract. The
preparer chooses N16/F32/F64 storage for each buffer from its whole content;
one executor serves collected and streamed output. The old accuracy selector
and Competitive implementation are removed. See [ADR 0034](adr/0034-one-interpreter-one-renderer.md),
[render options](../crates/pagura-render/src/options.rs) and
[ADR 0004](adr/0004-rendering-pipeline.md). This is a source description and
contract, not a new claim of measured accuracy for every document.

The existing [reference harness](../crates/pagura-reference/tests/harness/)
and [raster exact tests](../crates/pagura-raster/tests/exact/) check independent
coverage and the renderer across band heights, regions and store states. CI also
checks native/WASI parity. Previously reported fixture counts and rounding rates
are not promoted to this source revision without matching run evidence.
Extreme coordinates can still exhaust the bounded coverage certificate and
return `Limit`; this is distinct from accepting inaccurate pixels.

The editing tests and hosted interoperability job retain explicit independent
reopen checks. The recorded Poppler 24.02.0 failure on both the original and
saved `document/hybrid-precedence` fixture is a baseline incompatibility, not a
successful reopen; qpdf 11.9.0 accepts both. See [testing](TESTING.md).

The committed [corpus scoreboards](scoreboard/) identify their own older source
revisions, binary hashes and corpora. They are historical observations, not
current-head compatibility rates or cross-engine parity measurements. The old
aggregate table and timing ratios previously on this page were not reconciled
with those individual snapshots and have been removed. No replacement percentage
is asserted without a new run. Fixture counts likewise do not measure the
percentage of the PDF standard implemented.

The [requirement registries](../references/requirements/) retain normative
locators, tests and review status. Broad security, form, text, CMS and host
summaries were corrected where this audit established stale claims; the complete
future registry/API discoveries reopen the inventory. A `gaps` string or declared feature
tag alone is not a current capability verdict. The parity plan requires checking
live call sites and testing all new claims. A primary-text audit is
separate from an execution test and from corpus compatibility.

The [release matrix](../benchmarks/release-matrix.json) describes **benchmark
adapter coverage**, not product API availability. Its unimplemented operations
and unbound workloads still block the ADR 0002 performance claim. There is no
current claim of complete PDF conformance, competitor feature parity or universal
speed superiority.

## Overprint display policy

`RenderOptions::overprint` selects `OverprintPreview::{Auto, Always, Never}`;
Auto is the default. Auto simulates only when the document declares PDF/X print
intent: an `OutputIntents` entry with `S /GTS_PDFX` (14.11.5, ISO 15930), or a
`GTS_PDFXVersion` string in Info. Catalog and PDF 2.0 page output intents are
recognized, including indirect entries. A resolved page array takes precedence
over the catalog, even when empty; the Info declaration remains independent.
This detects a declaration, not PDF/X conformance; malformed advisory metadata
is treated as absent. Always enables
simulation wherever it can affect painted colourants. Never ignores overprint
and paints ordinary composite content, including in CMYK transparency groups
and soft-mask sources.
The CLI exposes `--overprint auto|always|never` for rendering and scanning.
The host protocol and viewer use Auto; a versioned render option and viewer
toggle are the next host step.

When enabled, simulation (8.6.7, 11.7.3–11.7.4) uses a CMYK process device if
an overprinting paint coexists with painted Separation or DeviceN colour, or
if an overprinting direct DeviceCMYK paint uses OPM 1. No-op flags keep the
ordinary rendering path and blending space. Explicit group blending spaces
still apply to process colour; spot plates pass through groups without colour
conversion, and soft masks always use their alternate spaces.

At page output, spot alternates supply CMYK ink equivalents, which are added
with saturation before RGB conversion. The first painted definition of a
colourant name supplies its alternate transform; channels sharing that
first DeviceN definition are transformed together. This is a display policy,
not a prediction of physical ink interaction. A simulated page supports at
most 32 spot plates; larger sets fail with `Limit`. Destination ICC output
transforms and fractional/pattern/per-colourant transfer selection remain
incomplete; whole-pixel transfer regions are implemented.

## Editing and signature boundaries

Changed incremental saves require a strict, unencrypted source. The default
signature policy rejects signed or uncertain input. `PreserveSignedBytes`
permits appending while preserving original bytes; it does not establish that
DocMDP/FieldMDP permits the edit or that a signature remains valid. Current
structural modification analysis is not yet connected to save enforcement.
Incremental saves retain previous values and therefore cannot sanitize redacted
content. See [Editing](EDITING.md) and [ADR 0031](adr/0031-editing-foundations.md).

PKI validation takes caller-selected anchors, time, policy and evidence; it does
not obtain trust from an embedded certificate or fetch a URL. Its implemented
path/CRL stages are separate from the PDF CMS integrity API. Signature results
must continue to distinguish integrity, revision coverage, permitted changes,
trust, validation time, revocation and algorithm policy (ADR 0015).

## Maintaining this inventory

A capability change updates its implementation, independent fixtures,
requirement entry and this page together. Record library, renderer, CLI, host
and viewer availability separately. Replace a historical scoreboard only with
an actual run that records the new source/binary/corpus identities. Unknown or
untested competitor cells remain open in the parity plan; they are never a pass.
