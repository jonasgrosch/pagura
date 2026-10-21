# Incremental performance stacking review

2026-10-21. Read-only source review; no combined candidate was built or measured
for this review. Existing results remain exploratory and do not admit either
optimization. The user has authorized incremental integration and evaluation.

## Inspected ancestry

The latest inspected local integration tip was `combine/images` at
`202bda8d79481e467951b57ceb780a803dde4a37`. A live `git ls-remote origin refs/heads/main` query confirmed
`752b52e7ff4633c9ad78d2a030498d3a281f33c1` on remote main. Local ancestry
checks show the newer combined stack is not included there. A local integration ref is not proof
that its changes have landed on remote main.

The relevant ancestry, oldest to newest, is:

1. `752b52e7`: certified signed-area glyph coverage.
2. `4534bd3c` (`combine/108`), then `7818503b`: byte composition and four-entry clip cache.
3. Image work through `87e68ccc`, then `ccd22246` (`combine/110`): glyph mask reservation and JPEG work.
4. `0dcf5cb9` (`combine/112`): register-based sequential JPEG block decoding.
5. `8a56cfdc`, `1a3cf0b9`, `039146b7`: glyph fragment lanes, packed blending and mask placement.
6. `f91f2286` (`combine/113`): avoid duplicate outline-set lookup during cache charging.
7. `d81576c9` (`combine/111`): nonoverlapping Competitive stroke pieces.
8. `202bda8d` (`combine/images`): retain expanded image interiors across the byte page.

Our inherited baseline `359e19dda5b78a23675678b6a153da7ab7fdc2c5` and this
stack are sibling lines with common ancestor
`a48e5ecc99059bf3b16873730f61d74834610d0b`. Our experiment baseline `676a872e`
has the same renderer/raster sources as `359e19dd`. Historical ratios therefore
compare different code from the current integration stack.

## Mechanical overlap

All source line anchors in this section refer to `202bda8d`, not an evolving checkout.

- **Viewport construction:** `crates/pagura-render/src/compositor.rs:419–430`
  still builds the viewport before choosing `soft_source`. Moving that construction
  inside the consuming branch is compatible by inspection. Preserve the four-entry
  clip cache at lines 201 and 406–418 and its publication at 463–464. The saved
  patch's preceding cache-hit context differs, so port the small change deliberately.
  Cache hits already avoid this work; only ordinary-source misses can benefit.
- **Stroke integration:** the approximate stroke selection at `compositor.rs:1010–1028`
  changes downstream outline construction. It does not remove viewport work.
  Keep soft-source geometry and cache clearing at lines 1438 and 1459 unchanged.
- **Glyph hull memo:** the removed `9ebac1cb` experiment reused finite root-run
  hulls across regions, after unchanged semantic charges. The current code still
  calls `regions::run_visible` at `compositor.rs:538–540`; `compositor/regions.rs:152`
  invokes `device_hull`, whose `display.rs:269` scans sources. The new glyph work
  has not replaced this calculation.
- **Glyph cache charging:** `fragments/glyphs.rs:144–147` reserves mask-table room;
  lines 191–214 avoid repeated outline-set insertion while accounting retained
  bytes. These concern Competitive mask retention, not root hulls or raster-work
  charges. A hull memo would still need its own admission, canonical-transform
  guards, root ordinals, and accounting review, including its former 2,064-byte
  object present even when disabled. Neither experiment implements sparse dispatch.

These changes can be composed at source level; this does not establish combined
correctness, memory acceptance, independent gains, or additive speedups.

## Fresh measurement design

Evaluate the viewport cleanup first. It removes unused work without adding cache
state. Evaluate the glyph memo separately afterward, retaining its evidence even
if its added state fails admission. Freeze each candidate before timing.

For each candidate, use a two-by-two design: old baseline, old baseline plus the
candidate, current stack, and current stack plus the candidate. Collect fresh
paired baseline/candidate samples within each source generation using the same
workload definitions, settings, toolchain and execution conditions. Historical
ratios are context, never a fresh control. Do not multiply old ratios or pool
pilot/confirmation results. The current-stack pair determines incremental value;
the old pair helps identify whether the opportunity changed after integration.

Record commits, source and binary hashes, harness/oracle identities, operation,
mode, DPI, cache state, sample order, complete outputs and runner/load evidence.
Freeze targets, protected cells, latency/tail/memory thresholds, sample counts,
statistics and stopping rules before observing timings. Preserve all attempts.
Contended results remain descriptive; passing numeric thresholds cannot repair
an execution-validity failure.

Verify each mode against independently derived expected output and its declared
rendering contract. Same-mode baseline byte equality is an additional invariant;
agreement with Exact alone is not an independent accuracy proof. Preserve
coverage, diagnostics, work limits, tile invariance and soft-mask crop semantics.
Run complete candidate validation; a check of the restored renderer cannot cover it.

## Proposed acceptance policy for a new experiment

A new policy could admit one useful, predeclared target gain when the other mode
is demonstrably neutral within a frozen margin. This is a proposal to review and
freeze before timing, not present admission. Define the minimum useful gain,
neutrality margin and uncertainty rule explicitly; failure to detect a regression
is not evidence of neutrality. All applicable accuracy, protected-workload,
tail-latency, memory and execution-validity requirements still apply.

Do not rewrite the [viewport plan](clip-viewport-plan.md), glyph-memo thresholds,
or their historical decisions to fit later evidence. Any new policy and dataset
receive a separate versioned record. The preserved partial gain is a reason to
test its incremental value, not a reason to claim the original gates passed.
