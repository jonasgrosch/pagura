# Production correspondence review

Reviewed candidate product fingerprint
`53062678753e6012a291b95dbf07cb8b069e7134ab836d587919975d1ed057c7`
against baseline `b3a124d48e4761b66a717edb542ceebf56ffa91b0e3f702f997b005d1a30a2c7`.
Diagnostic statements are excluded from this production review.

- PI-Q1: N16 has exactly three mutation sites: full constructor initialization,
  validated `Buffer::path` spans, and validated `MaskSink::N16` fragment rows.
  `Words.pixels` is private. `small_mask` and `unchanged_by` target only U8;
  dynamic paint rejects N16. Both painting sites widen the interval only after
  the existing validation. A failed path leaves it unchanged; a failure on a
  later fragment row retains the interval covering prior successful stores.
  `Buffer::output` remains dense and independent of the interval. Exhaustive
  65,536-word conversion identity and writer/error assertions are retained.
- PI-Q2: `render_chunk` is the sole production caller of `write_output`.
  `render_bands` initializes/refills the Pixmap to the same background before
  disjoint chunks. Relative origins, destination stride and row intersections
  are preserved. Tests cover multirow holes, colored backgrounds, irregular and
  offset public tiles across 256/512 seams, and painted/blank/painted storage
  reuse in both modes, including actual Competitive allocation reuse.
- PI-Q3: The nonopaque-background guard precedes Context creation and keeps
  alpha below 255 on F64. Converter arithmetic and precision admission are
  unchanged. Existing rational, background and both-mode regression tests pass;
  direct host precision additionally checks Exact native/WASI response parity.
- PI-Q4: Page construction still performs pixel validation, decoded admission,
  full-area charge, allocation and initialization in the original order.
  Interval output adds no charge, source evaluation or culling. All 47 ordered
  error/resource rows match baseline within each of the three platforms.
- PI-M1: Buffer, Option<Buffer>, Surface, MaskSink and Context have no relevant
  layout-affecting test-only fields. The temporary layout patch adds a test
  function, not state. The added Range fits the observed enclosing enum layout;
  no heap allocation is added, and the N16 payload remains count*8 bytes.
  Actual eight-type size/alignment observations agree in all six arms.

This is source correspondence plus bounded test/diagnostic evidence, not a proof
for every input. It does not establish process RSS, generated stack usage,
throughput, release admission or competitor standing. F32 callback overhead
remains protected by the unchanged performance matrix. R1's prediction failure
is preserved; independently frozen R2 verifies the padded submission geometry.
