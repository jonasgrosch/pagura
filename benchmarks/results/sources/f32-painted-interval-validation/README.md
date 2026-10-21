Source-only F32 interval six-arm diagnostic adaptation; unexecuted.

Use run_v3.py, prepare.py and the byte-identical run_wasi_diagnostic.mjs copy.
No historical helper or diagnostic patch is changed. prepare.py references the
archived layout-probe.patch and pi-q4-diagnostic.patch at their existing paths.
Their applicability to both new source trees is unobserved until the coordinator
performs the explicit combined precheck; no patch was applied during drafting.

Prerequisite: the uninstrumented F32 candidate must first pass its native tests
under the coordinator's runner/strict guard. This ordering remains coordinator-
enforced, not inferred from a test count or from an old N16 result. Only then
invoke prepare.py install under the same serial coordination.

Roots are fixed to baseline renderer-f32-interval-base/pagura at pristine
f9738174757252567730ff62b5d36620c01d1e6c and candidate renderer-n16-interval/pagura.
Evidence: candidate benchmarks/results/local/f32-painted-interval/
diagnostic-preparation-001/{before,installed,removed}.json and diagnostic-001/.

Installer changes are limited to transactional patch handling and new roots/
paths: check all hunks of BOTH archived patches together in each root before
changing either tree; then apply both in one git invocation per root. If an
apply fails after the transaction starts, reverse only the owned exact patches
on changed roots and verify the four source-file hashes match before the attempt.
Unknown rollback failure is retained and blocks further work. Precheck failure
leaves both trees untouched and retains before.json plus outer failure evidence.
There is no reset, broad checkout or overwrite of independent user changes.

Removal first verifies both complete installed source identities and unchanged
patch bytes, then prechecks both reverse patches in both roots before changing
either root. All four exact source files and the complete product identity must
match their respective preinstallation values before restored=true is written.
Failure during reversal rolls the owned transaction back to its installed state
when possible; no restored=true claim is made. Preserve failure receipts/files.
Installation/removal deliberately change source; their transaction records are
not source-unchanged validation receipts. The coordinator must bind those records
in the later clean quality receipt and check their full source correspondence.
Do not proceed with timing until removed.

The diagnostic runner retains its existing scoped run_process cancellation,
environment restoration, logs and failed/incomplete report behavior. A raised
cancellation can leave the current captured subprocess log unavailable; it never
counts as successful validation. The entire matrix remains six serial arms:
baseline/candidate × native/WASI SIMD/WASI scalar. Pass a NEW empty build-root
path; the helper creates separate arm/platform children to avoid artifact reuse.
Use --target-dir /private/tmp/f32-interval-diagnostics-001, for example, only if
that path is still absent. Do not share previous targets.

Run shape after successful installation and freezing installed.json's hash:

    <python> <absolute-run_v3.py> \
      --preparation <root>/benchmarks/results/local/f32-painted-interval/diagnostic-preparation-001/installed.json \
      --preparation-sha256 <SHA> \
      --output <root>/benchmarks/results/local/f32-painted-interval/diagnostic-001 \
      --target-dir /private/tmp/f32-interval-diagnostics-001 \
      --cargo <absolute-rustup-cargo-launcher> --node <absolute-node>

Always wrap with the existing runner and strict observer. Keep final receipts,
guards, exact invocation/source pins and raw artifacts. The Cargo argument must
be the launcher whose sibling rustup selects/pins the actual toolchain as in the
historical helper. Reject ambient build/runtime flag overrides before launch.

Unchanged checks: all lib tests (zero failures/ignored/filtered), exactly47
ordered Q4 records per arm, exactly8 layout size AND alignment rows, native/WASI
target+SIMD identity, exact same-platform Q4 transcripts, unchanged source and
generated fixtures, final helper/runtime/binary/artifact hashes. Layout deltas
<=0 are allowed; a decrease is not a measured RSS or peak-stack improvement.

Candidate-only required inventory now includes the renamed public test
tests::painted_interval::integer_rectangles_match_irregular_and_offset_tiles_in_both_modes,
the unchanged recycled-storage test, and five compositor::dynamic_interval_tests:
dynamic_consumers_mark_full_and_keep_independent_half_alpha_pixels;
disjoint_dynamic_operations_leave_the_interval_empty;
empty_shading_samples_still_conservatively_mark_the_dynamic_operation;
image_area_charge_failure_precedes_dynamic_preparation;
late_function_failure_keeps_full_range_partial_pixels_and_charges.

These new tests do not add Q4 print rows: the original47-record transcript stays
unchanged. Fresh candidate test counts may differ from baseline; explicit names,
zero-filter test summaries and retained binaries establish which code ran.
The old N16 marker names intentionally remain so archived patches/parsers are
reused byte-for-byte. Output experiment label is f32-interval-layout-pi-q4.

This is correctness/layout evidence only. No diagnostic build/test duration is
performance evidence, and no result exists until the coordinator runs it.
