# Conservative F32 page-output interval — source-only proposal

Status: design only. No product patch, imports, builds, tests, renders,
profiling or measurements were performed for this proposal. Finish the current
N16 retention decision first. **Before implementing this proposal, obtain a
fresh pinned F32 profile and inspect its output call chain/generated code.**
Separate renderer samples from harness/checksum work. If F32 output conversion
has no meaningful cost to remove, reject this direction before adding metadata.

Source reviewed: `/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura`,
product at HEAD `069d62d75c1a9a2756b2833169c4c314d1c14cab`, with the N16 interval
candidate. References below are relative to that root and identify the source
read; a later implementation must freeze and refresh its actual baseline.

This is potentially a larger next step than trimming N16's remaining row loop
or replacing F32's byte quantizer: it could eliminate entire untouched F32
output ranges, including sparse translucent text in both modes. That is an
architectural opportunity, not an established priority, speedup or RSS claim.
It does not remove buffer initialization, source validation, rasterization,
composition or their charges. Keep the three proposals as separate experiments.

## Scope and invariant

`compositor.rs:1672–1675` forces every nonopaque background to F64. At
`:1738–1744`, `Pixmap::with_storage` refills the output with the requested
background, including recycled storage (`pagura-raster/src/pixmap.rs:78–90`).
`compositor/narrow.rs:449–464` embeds the same opaque bytes exactly in F32.
Thus an unmodified narrow pixel and its initialized destination have identical
output bytes. This applies to current Exact and Competitive admission; it does
not make their raster coverage policies equivalent.

Retain all existing admission exclusions. Retained groups, soft masks,
transfers, separations and unsupported models use F64; admitted associative
groups share the page buffer (`compositor/plan.rs:58–74,147–161,204–217`;
`compositor.rs:1079–1099,1155–1159`). Do not generalize to F64 group storage or
translucent backgrounds. The direct opaque-image route either completes its
chunk or declines before output writes (`compositor/opaque_image.rs:85–160`),
so its fallback does not invalidate the initialized-destination assumption.

## Minimal implementation boundary, not an implemented patch

Use one plain `Storage<T>` containing `Vec<[T; 4]>` and `Range<usize>` for
N16/F32. No trait hierarchy, extra allocation, cache or dependency is needed.
Initialize the interval to `count..0`; keep eager pixel filling, arithmetic,
allocation/error order, and the dense `Buffer::output` contract unchanged.
Only page `write_output` may omit the conservative exterior
(`compositor/output.rs:31–59`). Holes inside the interval remain converted.

All F32 writes enter three APIs in `compositor/narrow.rs`:

- `Buffer::path` at `:542–553`: extend after the existing successful slice
  validation, before either the full fill or ordinary blend.
- `MaskSink::F32` at `:80–99`: reuse the validated row/column callback already
  used by N16. Extend only after that row's bounds and clipping succeed
  (`:718–744`). Zero-coverage halos may widen the interval. A later row failure
  must retain the extent of earlier successful writes.
- `Buffer::paint` at `:558–575`: preserve the pixel arithmetic and validation,
  but make its dynamic-source contract explicit as described next.

For dynamic sources, mark the **whole F32 buffer `0..pixels.len()` once per
operation**, immediately before its existing sampling loop. These are all three
production consumers:

1. `Context::paint_color`, covering admitted shading patterns:
   `compositor.rs:929–980`.
2. `Context::shading`: `compositor.rs:1294–1324`.
3. `Context::image`: `compositor/images.rs:63–104`.

Keep all source evaluation, sampling, transfer handling, work charges, index
checks and error propagation in their present order. The preparatory mark is
metadata-only and infallible. No per-pixel interval update or new release-mode
tracking branch belongs in these loops. Dynamic-source chunks deliberately
receive no output-skip benefit, protecting dense images from repeated min/max
bookkeeping. Even a no-op dynamic operation may conservatively mark everything.
For other storage formats this preparation must be a no-op, not a new format
error: an operation with no visited pixels must retain its existing behavior.
Keep any existing unsupported-format error at the original pixel-write point.

Rename/document the private pixel method as `paint_dynamic`, with a possible
`debug_assert` that its F32 interval is already the full buffer. This makes the
caller obligation visible without release overhead; it does not replace tests
or prevent future misuse by itself. A borrowing dynamic-painter API would
enforce more structurally, but would require broader Surface/index-loop
refactoring and is outside this proposed first slice.

## Required evidence and rejection criteria

- Preserve one allocation and F32 `count * 16` payload accounting, including
  `page_surface` admission, work charging and release (`compositor.rs:388–416`,
  `:1872–1873`). Verify actual native and WASI size/alignment of Buffer,
  Option<Buffer>, Surface, MaskSink and Context. A conventional wrapper would
  be 40 bytes on 64-bit and 20 on 32-bit targets, below the existing Bytes
  variant; changing MaskSink from a slice reference to a wrapper reference
  could also shrink it. These are static expectations, not Rust-layout proof.
- Extend empty/background, distant-write/holes, failed-span and late clip-row
  interval tests to F32. Retain `f32_fragment_masks_keep_path_arithmetic_for_every_coverage`,
  `f32_fragment_offsets_and_clip_order_keep_path_bits`, and
  `f32_fragment_admission_and_surface_bounds_are_checked`.
- Retain independent `colored_backdrop_and_nonzero_tiles_share_the_translucent_recurrence`
  and add translucent painted-then-blank output reuse plus irregular tile
  partitions in both modes. Dense Buffer::output must ignore interval metadata.
- Exercise **all three dynamic consumers**, not just images, with independent
  outputs, full-range marking and existing source/error/work-limit behavior.
  Include partial work followed by failure; do not skip zero-alpha evaluation
  or introduce a shortcut based on apparent source color equality.
- Reject/revise on any byte, alpha, tile, error/context, consumed-counter,
  allocation-admission or enclosing-layout regression. Keep nonopaque-background
  and retained-group fallback controls. Native/WASI SIMD/scalar validation and
  explicit helper/source/binary bindings are prerequisites to performance claims.
- If implementation is justified by the fresh profile, freeze a separate
  incremental experiment before timing, with sparse F32 text targets and dense
  image, shading, shading-pattern, N16 and F64 controls. Preserve every failure
  and RSS miss. Neither this source argument nor unchanged heap payload proves
  a time or peak-RSS benefit; no existing samples or admission decision transfer.
