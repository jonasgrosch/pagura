# PI-Q4: reuse existing checks, record identical-arm diagnostics

Source-only; unformatted, uncompiled and unexecuted. No active file changed.
`base/` contains four files from a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c;
`overlay/` and `diagnostic.patch` contain test-only instrumentation. Apply the
same temporary patch to dedicated baseline and candidate. It composes with the
layout probe outside `mod tests`; all changes here are inside existing test
modules. Remove exactly this patch and the layout probe, verify original source
inventories, and build timed binaries only after removal. No new dependency.

## Existing PI-Q4 coverage

Paths are relative to the renderer source root. No new fixture suite is needed.

| Exact test name | Existing evidence when it passes |
| --- | --- |
| `display::tests::opaque_colour_errors_survive_region_rejection_in_both_modes` (`display.rs:917`) | Both ICC/Type4 sources × visible/off-page/tile-miss/nested-off-page/nested-tile-miss; both modes; full Error equality for full/tile and modes, expected feature/object/position, empty strict diagnostics |
| `compositor::sources_tests::validation_and_paint_share_the_function_budget` | Source work 1 after validation, 2 after paint, next paint fails with work still 2 and the same positioned error/pixels |
| `compositor::sources_tests::validation_command_work_is_not_reset_before_execution` | Prevalidation reaches the exact command-work budget; subsequent execution fails without spending more or painting |
| `compositor::sources_tests::off_page_stroke_errors_do_not_apply_to_glyph_paint_metadata` | Both modes reject singular off-page Path, preserve its source position, and allow the corresponding Glyph paint metadata |
| `compositor::sources_tests::none_skips_source_painting_but_still_validates_path_strokes` | /None source is blank, but does not suppress singular stroke validation |
| `compositor::sources_tests::nested_validation_has_a_depth_bound_and_keeps_inner_context` | Depth2 fails at inner group41; depth3 succeeds |
| `compositor::sources_tests::off_page_stroke_inverse_overflow_remains_an_error` | Both modes preserve finite-inverse/nonfinite-mapped geometry failure and exact message/position |
| `compositor::sources_tests::transfers_stay_deferred_wide_and_reject_fractional_selection` | F64 transfer selection, off-region unevaluated transfer and positioned fractional-transfer Unsupported |
| `compositor::sources_tests::f32_fragments_convert_a_tint_once_per_run_before_painting` | Competitive fragment source consumes one function evaluation; next run fails at unchanged work/pixels |
| `compositor::tests::narrow_rectangles_charge_their_region_as_masks` (`compositor.rs:2578`) | Conservative 42-pixel region charged despite only24 covered; closed rectangle and dense-mask reference agree; exact successful raster-work limit and one extra unit fail |
| `compositor::index_tests::warmed_competitive_fragments_preserve_pixels_and_every_work_boundary` | Visible/off-tile warmed Competitive N16 fragment calls compare full Error, consumed work and partial bytes across every remaining-work boundary, memo enabled/disabled |
| `compositor::translucency_tests::competitive_f32_fragments_keep_offscreen_source_errors` | ICC/Type4 aggregate-intersecting but individually culled glyphs: Competitive keeps positioned full/tile errors; Exact intentionally remains successful blank |
| `tests::competitive_page_surfaces_check_the_pixel_budget_first` | Direct Competitive and Exact reject pixels before zero decoded-byte allowance with the fixed surface error |
| `tests::compositing_memory_work_and_shared_function_budgets_are_bounded` | Existing compile/render work, function, decoded-byte and command-count failures; successful F32 memory admission beside rejection |
| `tests::recycled_storage_is_counted_and_bounded_by_the_decoded_budget` | Recycled output is charged and oversized retained storage is refused |
| `compositor::tests::sole_rgb_images_preserve_work_and_surface_admission` | Protected image fast-path retains exact work boundary and storage failure semantics |

The two glyph modes intentionally have different source-conversion visibility
in the translucent offscreen case. Compare **same-mode baseline versus candidate**;
do not replace this retained contract with a new cross-mode equality assertion.

## One narrow missing boundary, plus temporary transcripts

New `compositor::tests::temporary_n16_resource_boundary_probe` fills the actual
page-surface gap: six N16 pixels require48 charged bytes. With synthetic prior
ledger `(work, allocated, function_work)=(11,7,13)`, exact limits
`pixels=6, decoded=55, work=17` succeed at `(17,55,13)`. Decreasing each one by
one preserves its complete expected Error and consumed counters. In particular,
the existing work failure occurs **after** assigning the48-byte page charge,
so its failed state is `(11,55,13)`; the probe does not change that ordering.
These are live ledger values, not allocator capacity, peak RSS or stack evidence.

The successful page checks blank output and then one opaque store at the last
pixel: independent colored bytes and all three counters remain unchanged by
`write_output`. This uses the shared N16 page helper, not a claim to exercise
both full public policy dispatches; the tests above cover those dispatches.

Existing tests gain only `PAGURA_PI_Q4_V1` prints (plus naming an already-tested
command error). They record complete Error Debug fields, source diagnostics and
function/command/depth/region counters. Assertions are retained unchanged. The
same patch never calls candidate-only `painted_range`, so it also fits a981.

## Coordinator execution without extra builds

Consolidate with the layout agent's six serial native/WASI SIMD/WASI scalar test
binary builds. Archive the Cargo-selected binary/module once per arm/platform;
run these filters against that retained artifact with `--nocapture --test-threads=1`:

```text
compositor::tests::temporary_n16_resource_boundary_probe             --exact
display::tests::opaque_colour_errors_survive_region_rejection_in_both_modes --exact
compositor::sources_tests
compositor::tests::narrow_rectangles_charge_their_region_as_masks     --exact
compositor::translucency_tests::competitive_f32_fragments_keep_offscreen_source_errors --exact
```

Native form: `/ABS/ARCHIVED-TEST FILTER --exact --nocapture --test-threads=1`.
Omit `--exact` for the `sources_tests` module. WASI form: pinned Node + existing
`run_wasi_tests.mjs /ABS/ARCHIVED.wasm` + the same test arguments. Require the
expected passing tests, not only exit0; retain stdout/stderr and executed module
hash/arguments. Extract the `PAGURA_PI_Q4_V1` suffixes in invocation order and
compare baseline/candidate on the same platform. The listed runs emit47 records;
missing records or any changed Error/counter/pixel field block PI-Q4 closure.
No elapsed-time text or allocator addresses belong in these comparisons.

Run the other existing boundary tests listed above as part of the focused or
full correctness check; do not manufacture additional limit sweeps where the
same exact assertion already exists. Ordinary command form, if not reusing the
archived binary, is `cargo test --locked --offline --release -j 1 -p pagura-render
--lib FILTER -- --test-threads=1`. Root alone executes all commands.

## Existing structural precision coverage and its limits

- `compositor::translucency_tests::full_coverage_half_alpha_keeps_the_unrounded_backdrop_in_both_modes`
  asserts N16 for opaque/F32 for half-alpha, then independently checks both modes.
- `compositor::translucency_tests::compiled_text_knockout_keeps_distinct_independent_answers`
  asserts F32 for TKfalse/F64 for TKtrue and checks all pixels in both modes.
- `compositor::translucency_tests::sub_byte_source_alpha_is_not_erased_before_composition`
  asserts F32 and an independent rational result in both modes.
- `compositor::plan::tests::opaque_and_translucent_sources_have_distinct_storage`,
  `opaque_text_elides_but_translucent_knockout_stays_wide`,
  `simulation_devices_and_transfer_feeds_stay_f64`, and
  `generated_general_path_stacks_select_the_proved_precision` cover planner
  formats/reasons, but the latter public render uses default Exact only.
- `display::tests::direct_root_backgrounds_remain_premultiplied_in_both_modes`
  covers both public modes/full/tile over alpha0/128/254/255 backgrounds. The
  post-plan F64 background override is at `compositor.rs:1674`; the test checks
  required bytes rather than exposing that private override directly.
- Private `compositor::narrow::mask_tests`, `compositor::output::tests` and
  `compositor::sources_tests::f32_fragments_convert_a_tint_once_per_run_before_painting`
  explicitly construct and exercise N16/F32 buffers; the last installs the
  Competitive fragment state. Public `tests::painted_interval` covers both
  modes/tile/reuse, but by itself does not identify the storage variant.

These tests are sufficient reusable semantic controls. They do **not** establish
actual N16 interval reach for every new benchmark fixture or count skipped
conversions. Keep that separate PI-REACH observation outstanding until the
coordinator's actual fixture/route probe is retained. Python matrix/oracle tests
check fixture bytes and inventory, not renderer dispatch. No new timing gate,
performance inference or admission follows from this diagnostic patch.
