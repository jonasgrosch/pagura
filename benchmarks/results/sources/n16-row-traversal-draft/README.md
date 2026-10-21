# Unapplied N16 row traversal trim

Possible separate follow-up only if the current painted-interval candidate is
retained. No production source was edited. No import, build, test, formatting,
render or profiling command was run. The patch has not been applied or tested.

Base: `/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura`, observed
HEAD `069d62d75c1a9a2756b2833169c4c314d1c14cab`; restored, uninstrumented
`crates/pagura-render/src/compositor/output.rs` Git blob
`f7acaa246c07449bdb087312f1eaac4f210fc451` (read-only hash, not just HEAD).

The patch changes only the N16 page-output row range. Its existing per-row
intersection and output body stay byte-for-byte unchanged. No metadata, heap,
layout, buffer initialization, source validation, work charge, quantizer or
dense `Buffer::output` behavior changes. F32/F64 routes are untouched.

Safety cases:

- Keep the guard after existing output stride/origin validation.
- `painted.is_empty()` includes reversed constructor state `count..0`; test it
  before `end - 1`. The width-zero guard preserves the old no-store behavior.
- Nonempty exclusive end uses `(end - 1) / width + 1`: a row-aligned end does
  not introduce the next row, and there is no `end + width - 1` overflow.
- Clamp both row bounds to height. The unchanged intersection body preserves
  partial endpoint rows and all holes between widely separated writes.
- Divide by the chunk width. Continue using the existing output stride and
  origin for destination addressing; they can differ on offset tiles.

Existing exact test names to reuse, without adding a new test framework:

- `compositor::output::tests::narrow_page_output_preserves_colored_background_across_chunk_edges`
  covers empty `24..0`, width 6, a store ending exactly at 12, the next row's
  first column, colored backgrounds, nonzero origins and unchanged counters.
- `compositor::output::tests::n16_page_output_preserves_holes_in_a_multirow_painted_interval`
  covers range `2..26` in a 6-by-5 chunk, partial endpoint rows, interior holes,
  different destination stride, offset x/y and unchanged counters.
- `tests::painted_interval::opaque_integer_rectangles_match_irregular_and_offset_tiles_in_both_modes`
- `tests::painted_interval::context_painted_then_blank_output_refills_recycled_storage_in_both_modes`
- `compositor::narrow::mask_tests::word_output_matches_rational_rounding_for_every_word`
  retains the dense arbitrary-destination output contract.
- `compositor::narrow::mask_tests::n16_fragment_interval_keeps_prior_rows_when_a_later_clip_row_fails`
- `compositor::narrow::mask_tests::n16_distant_fragment_rows_keep_the_holes_and_full_interval`

The source-level saving is fewer row-intersection iterations on empty/sparse
N16 chunks. Converted pixels/calls are unchanged. Dense/distant intervals save
no iterations and incur the new quotient/bound calculations. This is not a
measured speedup and must not alter or reuse the current confirmation evidence.
