# F32 preservation: retained accuracy improvement

The renderer now keeps the planner-admitted F32 precision in Competitive mode. An F32 fragment-mask sink preserves cached glyph coverage and once-per-run source conversion. The known translucent-text error is corrected. This is an accuracy improvement with a descriptive cost diagnostic, not a new speed or release admission.

The baseline is product commit `c70b4678`; the candidate is the F32 product overlay on `e45a3308`. Exact source and native binary hashes are in `f32-preservation-cost-bindings.json`. The parked color-allocation patch was absent from both measured arms.

All 1,406 candidate outputs (preflight plus warmup/measured outputs) match the fixed independent RGB references with zero error, correct dimensions and alpha 255. The baseline has the same 37 known failures: 192 channels at 63 rather than 64 in Competitive text-knockout-false. Every other baseline output passes. This does not certify arbitrary PDFs or transparent output.

The one frozen trial used 19 cases ×2 modes, five warmup and 31 measured pairs per cell, balanced randomized order (seed 20261110). All 2,736 run outputs and 76 preflight outputs were retained and independently audited. The metric is in-memory open+compile+retained-render time, excluding startup, file I/O, output emission and destruction. The cost run had 18 observer scans with no known competing job. It was not repeated.

- Exact: median of case time ratios **0.9942**; case range 0.9750–1.0100. Median peak-RSS ratios range 0.9948–1.0179.
- Competitive: median of case time ratios **0.9951**; case range 0.9673–1.0106. Median peak-RSS ratios range 0.9947–1.0144.

Ratios below 1 favor the candidate. These small variations do not establish a speed win; there are no confidence bounds or tail gate. The known-wrong baseline cell describes the cost of correcting accuracy, not equivalent-output superiority. Whole-process RSS includes code/startup and does not establish equal buffer layout or memory behavior on larger pages.

| Case | Mode | Baseline µs | Candidate µs | Time ratio | Median RSS ratio | Max RSS ratio |
|---|---|---:|---:|---:|---:|---:|
| classic/blank | exact | 56.21 | 54.83 | 0.9755 | 1.0145 | 1.0286 |
| classic/blank | competitive | 57.58 | 57.21 | 0.9935 | 1.0144 | 1.0140 |
| classic/rectangle | exact | 72.58 | 73.04 | 1.0063 | 1.0064 | 1.0190 |
| classic/rectangle | competitive | 74.33 | 73.96 | 0.9949 | 1.0064 | 1.0063 |
| classic/graphics-state | exact | 76.67 | 76.33 | 0.9956 | 1.0064 | 1.0062 |
| classic/graphics-state | competitive | 78.29 | 79.12 | 1.0106 | 1.0064 | 0.9938 |
| classic/negative-rectangle | exact | 72.54 | 72.58 | 1.0006 | 1.0064 | 0.9938 |
| classic/negative-rectangle | competitive | 75.38 | 73.25 | 0.9718 | 1.0064 | 1.0126 |
| classic/inherited-box | exact | 72.42 | 72.00 | 0.9942 | 1.0065 | 1.0191 |
| classic/inherited-box | competitive | 72.71 | 72.96 | 1.0034 | 1.0065 | 1.0000 |
| classic/clipped-to-page | exact | 72.54 | 71.96 | 0.9919 | 1.0064 | 1.0063 |
| classic/clipped-to-page | competitive | 74.33 | 74.08 | 0.9966 | 1.0064 | 1.0063 |
| classic/many-rectangles | exact | 1197.67 | 1184.79 | 0.9892 | 1.0031 | 1.0318 |
| classic/many-rectangles | competitive | 1387.46 | 1383.71 | 0.9973 | 1.0031 | 1.0030 |
| classic/content-array-split | exact | 78.46 | 76.58 | 0.9761 | 1.0064 | 0.9816 |
| classic/content-array-split | competitive | 79.92 | 79.00 | 0.9885 | 1.0064 | 1.0189 |
| classic/many-operands | exact | 13112.04 | 12832.17 | 0.9787 | 1.0008 | 1.0061 |
| classic/many-operands | competitive | 16124.42 | 15840.00 | 0.9824 | 1.0008 | 1.0085 |
| paint/text-heavy-page | exact | 516.25 | 516.50 | 1.0005 | 1.0000 | 0.9880 |
| paint/text-heavy-page | competitive | 343.00 | 345.29 | 1.0067 | 1.0042 | 0.9919 |
| paint/text-mode-4 | exact | 165.04 | 163.79 | 0.9924 | 0.9948 | 0.9848 |
| paint/text-mode-4 | competitive | 165.62 | 164.46 | 0.9930 | 1.0000 | 0.9897 |
| paint/type3-d0-colour | exact | 120.96 | 122.17 | 1.0100 | 1.0111 | 1.0054 |
| paint/type3-d0-colour | competitive | 122.62 | 122.46 | 0.9986 | 1.0111 | 1.0054 |
| paint/text-knockout-true | exact | 173.96 | 173.79 | 0.9990 | 1.0000 | 1.0100 |
| paint/text-knockout-true | competitive | 177.67 | 175.25 | 0.9864 | 1.0000 | 1.0050 |
| paint/text-knockout-false | exact | 160.92 | 161.92 | 1.0062 | 1.0000 | 0.9747 |
| paint/text-knockout-false | competitive | 158.17 | 158.38 | 1.0013 | 0.9947 | 0.9845 |
| paint/image-soft-mask | exact | 92.04 | 91.21 | 0.9909 | 1.0179 | 1.0058 |
| paint/image-soft-mask | competitive | 95.46 | 92.33 | 0.9673 | 1.0118 | 1.0117 |
| paint/image-direct-binary-halves | exact | 84.96 | 82.83 | 0.9750 | 1.0122 | 1.0241 |
| paint/image-direct-binary-halves | competitive | 84.58 | 84.17 | 0.9951 | 1.0122 | 1.0238 |
| paint/image-direct-gray-quarter-turn | exact | 84.88 | 85.38 | 1.0059 | 1.0122 | 0.9941 |
| paint/image-direct-gray-quarter-turn | competitive | 86.04 | 86.25 | 1.0024 | 1.0122 | 1.0120 |
| paint/image-direct-rgb-fractional | exact | 89.33 | 88.17 | 0.9869 | 1.0120 | 1.0000 |
| paint/image-direct-rgb-fractional | competitive | 90.08 | 89.04 | 0.9884 | 1.0120 | 1.0119 |
| paint/image-direct-bilinear | exact | 84.54 | 84.88 | 1.0039 | 1.0121 | 1.0178 |
| paint/image-direct-bilinear | competitive | 85.71 | 86.12 | 1.0049 | 1.0121 | 1.0299 |

Validation: the full `python3 -B tools/check.py --jobs 1` passed, including workspace tests, lint/docs, native/WASI host checks, 196 tooling tests and 1,249 conformance cases. The focused release renderer suite passed 226 tests. The same compiled WASI test artifact passed 104 compositor tests plus the pixel-before-decoded-budget test. Raw-bit mask parity, literal alpha recurrences, clipping order, colored backdrop/tile boundaries, cache work boundaries, source diagnostics and once-per-run function work are covered.

The initial renderer attempt retained one failure because the direct private error message changed from page to surface after selecting the shared output allocator. The final test preserves the pixel 16 limit, adds decoded-bytes 0, and checks pixel admission first in both modes. No cap or independent pixel reference was relaxed. An initial build launch used an invalid runner kind and launched no build; its idle receipt is preserved. No measured trial was retried.

Limits: the fixed matrix does not cover large dynamic/shading performance, browser or WASI timing. F32 chunks can repeat traversal and source evaluation on wide pages and consume different memory than byte bands. Native/WASI MaskSink layout was not measured. Coverage, image/font and shading policies still differ between modes. No claim of global perfection, full mode unification or closing the Hayro speed gap follows.

Next separate performance slice: reserve the five rectangle path segments once, then measure incrementally on this retained stack. The color-component reuse patch remains available for a later independent experiment. Raw evidence is in `local/f32-preservation/`; the JSON companion retains all 38 cells, source identities and completed receipt/guard hashes.
