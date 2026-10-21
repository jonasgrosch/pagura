# Image-direct RGB8 reference policy, version 1

This policy applies only to the four independently authored
`paint/image-direct-*` fixtures listed in
[the derivation](image-direct-derivation.md). It defines the discrete sampling
model used by their existing expected pixels. It is not a general PDF
conformance certificate or a proof that either renderer mode meets that model
for arbitrary images. No PDF, oracle pixel, or existing tolerance is changed by
adding this metadata.

## Source and invocation

Each fixture is one self-contained, valid, unencrypted PDF page. It contains
one opaque, unfiltered image XObject with DeviceGray or DeviceRGB samples; no
external resources, fonts, substitution, ICC/default color-space profiles,
transfer functions, masks, matte, separations, transparency groups or optional
content. There are no annotations, forms, page rotation or nondefault UserUnit.
The effective MediaBox and CropBox are both `[0 0 32 32]`. The CropBox is absent
and therefore defaults to the MediaBox; UserUnit is 1 and rotation is 0.

The reference is page index 0, at 72 DPI, with a white opaque background and
annotations enabled. Output is top-to-bottom, interleaved RGB8 P6, maxval 255.
The CLI composites its output over white. This observes RGB only; it provides
no independent assertion about alpha channels or other backgrounds.

Conformance invokes `render --accuracy MODE --overprint auto INPUT OUTPUT 72 0`,
without `--strict` or a password. Recovery is the CLI's repair mode, but these
sources require no repairs or informational diagnostics and also open strictly.
All sample colors are device RGB/gray; no ICC conversion or rendering-intent
override participates. Auto overprint has no effect on these opaque process-color
sources. Font-provider identity does not affect them because they have no fonts.

Both `render` and the CLI's `convert` command use `RenderContext::render_page`.
Their operation and page-range identities remain separate execution-receipt
fields. Exact uses full compilation validation; Competitive uses drawing-only
compilation validation. Retained `compile_page` plus `DisplayList::render` is a
different entrypoint and has no record in this profile. Other pages, DPI values,
tiles, output formats, page boxes, annotations policies, backgrounds or
compilation scopes require a separately reviewed profile.

## Numerical model

Packed sample codes are decoded to exact integer channel levels. For the
admitted 1-, 2- and 8-bit depths, `255 / (2^bits - 1)` is an integer. The inverted
gray fixture applies `[1 0]` Decode, so its level is 255 minus the ordinary
level. Gray is repeated into all three channels. No lossy codec is involved.

The image placement has rational coordinates and is axis-aligned or a quarter
turn. Minification uses the exact area-weighted mean of source-cell levels in
the covered part of each pixel. The magnified `/Interpolate true` fixture uses
bilinear center sampling, clamping indices to the image edge. The precise
formulas and source patterns are recorded in the derivation.

Let `A` be the exact area of the image's footprint within one unit pixel. Its
coverage byte is `C = floor(255 A + 1/2)`. A pixel with `C = 0` is white. For
each channel with sampled level `V`, the final expected byte is
`floor((C V + (255 - C) 255) / 255 + 1/2)`.
This deliberately includes the existing coverage-byte quantization. It does
not substitute a continuous-scene area average or reorder sampling and color
operations. All oracle arithmetic in these formulas is rational; final ties
round upward.

## Frozen bounds and provenance

The Exact bound is zero for all four fixtures. The Competitive bound is zero
for binary-halves, gray-quarter-turn and bilinear; it is the existing one
channel level for rgb-fractional, as recorded in
`tools/fixturegen/competitive.py`. That measured compatibility allowance is not
a proof for an entire kernel or an invitation to raise a failing tolerance.
The profile rejects other bounds even if both a case and its metadata are
changed together. Changes require a new reviewed policy decision.

Schema-2 fixture entries carry this profile and separate schema-1 reference
records and canonical metadata hashes for both modes. The generated hashes
bind source PDF bytes, expected P6 bytes, this policy, the derivation, and the
reviewed repository-local generator/helper/writer sources. Regenerating these
records is not approval. The frozen manifest and its source changes must be
reviewed independently of candidate output. Runtime source rehashing includes
both documents, even when a broader source-tree digest excludes documentation.

The source list includes the fixture module's repository-local imports and the
binding/verifier helpers; it does not claim a hermetic Python or OS environment.
The oracle uses no Pagura executable, product writer, product decoder or
competitor pixels. The derivation and source set still require review: a hash
cannot establish mathematical validity or independence by itself.
