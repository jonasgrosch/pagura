# Capability inventory — G00 review view

Generated from [capabilities.json](../benchmarks/capabilities.json) by
`uv run --locked --offline python tools/capabilities.py`. Edit the ledger, not this file.

Source audit: `19059e68448288ccad84a3bc2f4c6c69c5142e2f`. G00 is in progress.
These are scoped source observations, not executed feature tests or a complete inventory.
Library includes lower-level crate APIs; each row states its remaining public integration.
Linked tests/fixtures exist; listing them does not claim a fresh pass.
All competitor cells remain unprobed. Schema 1 intentionally cannot record runtime
support or N/A until G01 defines the probe evidence contract.

Source hashes cover UTF-8 text with LF-normalized newlines for checkout portability;
they are not executable/binary identities. Regeneration never refreshes evidence hashes.

[Plan and acceptance gates](COMPETITIVE_PARITY.md) · [Capability summary](STATUS.md)

## Source observations

| ID / package | Feature | Library / CLI / host / viewer | Existing support | Remaining |
|---|---|---|---|---|
| `document.structure` / R01 | PDF file structures and reported recovery | partial / partial / partial / partial | Classic/xref streams, object streams, hybrid revisions, page trees and reported reconstruction. | Current-corpus recovery classification; external resources require a host contract. |
| `document.range-input` / D01 | Range and linearization-aware loading | missing / missing / missing / missing | Complete-buffer opening, lazy object caches and immutable source bytes are available. | Immutable range sources, missing-data state, scoped loading, linearization and resumable host jobs (ADR 0035 proposed). |
| `security.passwords` / D03 | Password security and encoding | partial / partial / partial / partial | Standard security R2-R6, RC4/AES and R6 SASLprep; CLI supports legacy text conversion. | Public-key handlers, encrypted history and browser legacy text conversion; writing encryption is E01. |
| `fonts.programs` / R04 | Embedded font programs | partial / partial / partial / partial | TrueType/OpenType glyf, Type 1, CFF Expert/ExpertSubset and FDSelect 4; default CFF2 blends. | Remaining formats, variations, executable programs and policy-controlled hinting; see the font inventory. |
| `fonts.cid-substitution` / R04 | CID font substitution | partial / partial / partial / partial | Provider-backed horizontal substitution for the supported Adobe collections; embedded vertical text works. | Vertical substituted glyph forms/metrics and unavailable provider/collection coverage. |
| `images.codecs` / R03 | PDF image codec variants | partial / partial / partial / partial | JPEG sequential/progressive with Huffman/arithmetic, CCITT G3/G4, JBIG2 regions and JPEG 2000 Part 1. | Inventory valid residual JPEG, CCITT, JBIG2 and JPX variants rather than interpreting generic filter tags as total absence. |
| `images.jpx-alpha` / R02 | JPX embedded straight/premultiplied alpha | implemented / implemented / implemented / implemented | SMaskInData 1/2 decode and compositing, including alpha normalization/unpremultiplication. | Indexed opacity/channel mapping and additional colour/layout interactions are separate remaining R02 work. |
| `images.jpx-colour` / R02 | JPX embedded colour interpretation | partial / partial / partial / partial | Explicit PDF colour spaces and embedded Gray/RGB/CMYK mapping. | Embedded ICC/sYCC and remaining precedence/channel-layout integration. |
| `colour.transparency` / R05 | Group colour, transparency and overprint | partial / partial / partial / partial | Gray/RGB/CMYK groups, knockout, blend modes, masks, forward ICC and process/spot overprint display. | Destination/inverse ICC, calibrated group destinations, black-point compensation and transfer-region combinations. |
| `render.graphics` / R06 | Graphics combinations and exact rendering | partial / partial / partial / partial | One interpreter, prepared scenes, per-buffer formats and one exact band executor; paths, clips, shadings and patterns. | Valid singular-transform/pattern refusals and independent accuracy evidence for difficult geometry. |
| `annotations.appearances` / V01 | Annotation appearance synthesis | partial / partial / partial / partial | Stored appearances, Link/Square borders, Text icons and supported empty widgets. | Other subtype appearances, richer borders, nonempty/rotated widgets and interaction states. |
| `forms.workflow` / V02 | AcroForm filling and persistence | partial / unreviewed / partial / partial | Text/choice/button filling, appearance generation, saved values and appearances, browser preview/discard/download. | Rich text, broader encodings/transforms, generated button states and nested direct fields; XFA belongs to V03. |
| `interaction.scripts-xfa` / V03 | XFA and scripted actions | missing / missing / missing / missing | Actions are inert views; XFA is detected; ordinary AcroForms have separate support. | Explicit host capability/ADR decisions, XFA layout/data/events and baseline-supported script/action subsets. |
| `text.accessibility` / V04 | Text extraction and tagged accessibility | partial / partial / partial / partial | Geometric text/search, replacement text, a separate tagged logical view and a screen-reader-readable DOM layer. | Logical-view integration into viewer accessibility, namespaces, remaining replacements and bidi/script reconstruction. |
| `viewer.layers-printing` / V05 | Optional content and print/export controls | partial / partial / partial / partial | Optional-content evaluation with View usage; library/CLI overprint choices. | Layer UI/configuration and Print/Export usage, print preferences and host/viewer overprint control. |
| `render.band-output` / D01 | Band output and cooperative stopping | implemented / partial / missing / missing | RenderContext lends final RGBA bands after preflight; fallible consumers and stops between bands; CLI streams PPM. | Persistent pause/resume, host active-request cancellation and async backpressure; calls currently restart. |
| `export.png` / D02 | PNG/zlib output integration | partial / missing / missing / missing | Whole-buffer PNG and DEFLATE encoders in the codec crate. | Facade/CLI PNG output and incremental PNG/DEFLATE; extend existing algorithms and band output. |
| `export.image-assets` / D02 | Public image asset extraction | missing / missing / missing / missing | ImageSource describes encoded images and defers sample decoding for rendering. | Asset/resource/occurrence inventory, exact-stream eligibility and normalized isolated-image export APIs. |
| `document.services` / D02 | Typed document services | missing / missing / missing / missing | Low-level immutable object access and typed navigation exist. | Typed metadata/XMP, embedded/associated files, portfolios, thumbnails and their public workflows. |
| `write.rewrite-encryption` / E01 | Writing, full rewrite and encryption output | partial / unreviewed / unreviewed / unreviewed | Deterministic new PDF 1.7 output and classic/xref-stream incremental revisions. | Full rewrite, object deletion/streams, PDF 2.0 output and encryption-on-write; modified saves currently require strict unencrypted input. |
| `editing.general` / E02 | General document editing | partial / unreviewed / partial / partial | Immutable transactions, rollback, no-op byte preservation and form editing. | Typed page/content/annotation authoring, merge/split, resource remapping, undo/redo and associated service edits. |
| `editing.redaction` / E03 | Sanitizing redaction | missing / missing / missing / missing | Incremental edits preserve prior revisions; they provide no sanitization guarantee. | Content-aware removal plus full rewrite and independent hidden-content/canary scans. |
| `signatures.structure` / S01 | Historical signatures and structural change analysis | partial / partial / unreviewed / missing | Historical discovery, ByteRange/revision coverage and structural DocMDP/FieldMDP classification. | Encrypted/repaired history and remaining transforms/exceptions; integrate results with save permission enforcement. |
| `signatures.trust` / S01 | CMS integrity and PDF trust integration | partial / partial / unreviewed / missing | CMS/CAdES/RFC3161 integrity; separate host-anchored PKI path and offline complete/indirect/delta CRL APIs. | PDF results still leave trust/time/revocation unchecked; connect real crypto and add OCSP, TSA and PAdES/LTV policy. |
| `signatures.creation` / S02 | PDF signature creation | missing / missing / missing / missing | Verification-only primitives and original-byte-preserving incremental infrastructure. | ADR for host signing provider, reservations, CMS/CAdES creation, timestamps and independent validation. |
| `profiles.validation` / C01 | PDF version/profile conformance | missing / missing / missing / missing | Version headers and PDF/X print-intent declarations are recognized; this is not profile validation. | Explicit PDF/A, PDF/UA, PDF/X and baseline-supported extension validators and conforming creation gates. |

## Baseline probes

| Baseline | Unprobed capability cells |
|---|---:|
| pdfium | 26 |
| mupdf | 26 |
| pdfjs | 26 |
| poppler | 26 |
| hayro | 26 |
| quartz | 26 |

## Feature-tag review queue

This lexical scan finds literal `Feature::new` uses under crate `src` directories,
excluding separate test files. Inline tests/comments may be included; macros, dynamic
tags and untagged API gaps require manual review. A declaration is neither a reachable
refusal nor a missing feature. Classify its call sites before assigning feature work.

| Candidate | Source declarations | Review |
|---|---|---|
| `annotation.appearance` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.border-style` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.default-appearance` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.font-encoding` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.text-matrix` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.update` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `appearance.widget-rotation` | [crates/pagura-appearance/src/features.rs](../crates/pagura-appearance/src/features.rs) | unreviewed |
| `asn1.constructed-type` | [crates/pagura-pki/src/lib.rs](../crates/pagura-pki/src/lib.rs) | unreviewed |
| `asn1.universal-type` | [crates/pagura-pki/src/lib.rs](../crates/pagura-pki/src/lib.rs) | unreviewed |
| `ccitt.extension` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `colorspace.BlackPoint` | [crates/pagura-color/src/features.rs](../crates/pagura-color/src/features.rs) | unreviewed |
| `colorspace.ICCBased` | [crates/pagura-color/src/features.rs](../crates/pagura-color/src/features.rs) | unreviewed |
| `colorspace.unknown` | [crates/pagura-color/src/features.rs](../crates/pagura-color/src/features.rs) | unreviewed |
| `document.historical-encryption` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `document.historical-source` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `edit.button-appearance` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `edit.direct-field` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `edit.encrypted` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `edit.password` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `edit.repaired` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `edit.signed` | [crates/pagura/src/editing.rs](../crates/pagura/src/editing.rs) | unreviewed |
| `extgstate.UseBlackPtComp` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `filter.CCITTFaxDecode` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs) | unreviewed |
| `filter.DCTDecode` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs), [crates/pagura-core/src/error.rs](../crates/pagura-core/src/error.rs) | unreviewed |
| `filter.JBIG2Decode` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs) | unreviewed |
| `filter.JPXDecode` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs), [crates/pagura-image/src/jpx/mod.rs](../crates/pagura-image/src/jpx/mod.rs) | unreviewed |
| `filter.unknown` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs) | unreviewed |
| `font.cid-substitute-vertical` | [crates/pagura-text/src/features.rs](../crates/pagura-text/src/features.rs) | unreviewed |
| `font.cmap-program` | [crates/pagura-text/src/features.rs](../crates/pagura-text/src/features.rs) | unreviewed |
| `font.format` | [crates/pagura-font/src/lib.rs](../crates/pagura-font/src/lib.rs) | unreviewed |
| `font.non-embedded` | [crates/pagura-text/src/features.rs](../crates/pagura-text/src/features.rs) | unreviewed |
| `font.predefined-cmap` | [crates/pagura-text/src/features.rs](../crates/pagura-text/src/features.rs) | unreviewed |
| `font.type1-postscript` | [crates/pagura-font/src/lib.rs](../crates/pagura-font/src/lib.rs) | unreviewed |
| `font.variations` | [crates/pagura-font/src/lib.rs](../crates/pagura-font/src/lib.rs) | unreviewed |
| `form.field-type` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `form.rich-text` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `form.xfa` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `function.calculator` | [crates/pagura-color/src/features.rs](../crates/pagura-color/src/features.rs) | unreviewed |
| `graphics.singular-stroke` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `image.JPX-colour` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `image.JPX-sample-layout` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `image.embedded-alpha` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `image.inline-colour-length` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `jbig2.extended-template` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.extension` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.halftone-region` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.huffman-table` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.mmr-extension` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.page-combination` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.pattern-dictionary` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.profile` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.refinement-region` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.segment-type` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.symbol-context` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.symbol-dictionary` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.symbol-refinement` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.text-layout` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.text-refinement` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.text-region` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jbig2.unknown-length` | [crates/pagura-image/src/jbig2/mod.rs](../crates/pagura-image/src/jbig2/mod.rs) | unreviewed |
| `jpeg.arithmetic` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `jpeg.components` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `jpeg.precision` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `jpeg.process` | [crates/pagura-image/src/features.rs](../crates/pagura-image/src/features.rs) | unreviewed |
| `optionalcontent.usage` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `pattern.ExtGState` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `pattern.image-mask` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `pattern.tiling` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `predictor.dimensions` | [crates/pagura-codec/src/lib.rs](../crates/pagura-codec/src/lib.rs) | unreviewed |
| `raster.singular-pen` | [crates/pagura-raster/src/features.rs](../crates/pagura-raster/src/features.rs) | unreviewed |
| `security.Adobe-PubSec` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `security.CFM` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `security.V3` | [crates/pagura-crypto/src/lib.rs](../crates/pagura-crypto/src/lib.rs) | unreviewed |
| `security.crypt-length` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `security.password-encoding` | [crates/pagura-cli/src/password.rs](../crates/pagura-cli/src/password.rs) | unreviewed |
| `security.revision` | [crates/pagura-crypto/src/lib.rs](../crates/pagura-crypto/src/lib.rs) | unreviewed |
| `security.unknown` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `security.version` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `shading.transform` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `signature.algorithm` | [crates/pagura/src/signatures.rs](../crates/pagura/src/signatures.rs) | unreviewed |
| `stream.external` | [crates/pagura-document/src/features.rs](../crates/pagura-document/src/features.rs) | unreviewed |
| `text.structure-content` | [crates/pagura-extract/src/features.rs](../crates/pagura-extract/src/features.rs) | unreviewed |
| `text.structure-namespace` | [crates/pagura-extract/src/features.rs](../crates/pagura-extract/src/features.rs) | unreviewed |
| `text.structure-replacement` | [crates/pagura-extract/src/features.rs](../crates/pagura-extract/src/features.rs) | unreviewed |
| `transparency.BM` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `transparency.group` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `transparency.group-colorspace` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `transparency.transfer` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `xobject.jpx-color` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
| `xobject.subtype` | [crates/pagura-interpret/src/features.rs](../crates/pagura-interpret/src/features.rs) | unreviewed |
