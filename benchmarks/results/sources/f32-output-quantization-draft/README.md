# F32 output quantizer: unapplied source-only draft

This is a separate follow-up hypothesis from the committed
`docs/plans/f32-output-quantization-research.md`. Nothing has been applied,
imported, built, tested, formatted, rendered or benchmarked. No performance
claim or implementation promotion follows from this patch. Finish the current
N16 retention decision first; then inspect a pinned F32 output profile and
native/WASM disassembly before deciding whether this deserves an experiment.

## Exact scope

`candidate.patch` changes one call in `Buffer::output`'s F32 arm from
`pixel.map(byte)` to `pixel.map(float_byte)` and adds one private helper:

```rust
(value + f32::from_bits(0x3eff_ffff)) as u8
```

The original `byte()` is also called by `byte_source()` for all three source
RGB components and alpha (`narrow.rs:820–825`). It is deliberately unchanged.
`scalar()`, source `level()` with its odd-significand correction, `lerp()`,
fragment/path producers, N16/U8 conversion, clipping, admission, charges and
errors are unchanged. The output loop keeps the same array map, source slicing,
complete-pixel truncation and overlong-destination behavior; no new flattening,
storage, cache, trait, allocation, unsafe code or dependency is introduced.

## Independent arithmetic argument

The bias is exactly `1/2 - 2^-25`, the f32 predecessor of 1/2. For each byte
transition n=1..255, `t=(2*n-1)/2` is exactly representable. With nearest-even
f32 addition, `t+bias` rounds to n; the preceding f32 plus bias rounds to the
f32 immediately below n. At n=1 the exact sum is the midpoint below 1, whose
even result is 1. At larger transitions the bias subtraction is smaller than
the relevant half spacing. The succeeding f32 remains on the n side.
Monotonic addition and truncation therefore preserve every finite byte interval.
Negative values saturate to 0; values at/above the upper interval saturate to 255.
Positive/negative infinity retain 255/0, and every NaN converts to 0.
Ordinary `+0.5` would fail for the predecessor of 0.5, which this draft tests.
This source argument does not replace actual native and WASI validation.

## Three focused tests

All new tests exercise actual `Buffer::F32::output`; none computes its expected
result with the old converter or the proposed helper.

1. All 255 predecessor/exact/successor half transitions expect the independently
   specified bytes n-1/n/n, plus an integer lane n.
2. Every integer 0..255 and literal signed zero/subnormal/normal/extreme/infinity
   encodings, both signs and varied quiet/signaling NaN payloads, have fixed
   byte expectations.
3. Distinct channels, offset 1, destination lengths 0..13, partial pixels,
   overlong destinations, and the valid empty source-end offset verify literal
   bytes and unchanged sentinel tails.

Retain existing `source_encoding_preserves_byte_rounding_at_half_levels`,
`rational_normal_stacks`, long-stack, fragment-bit/clip-order and output tests.
If root elects to apply later, run the three `f32_output_` tests and then the
renderer library suite on native, WASI SIMD and WASI scalar using the existing
owned runner, pinned toolchain/wrapper and distinct arm/platform target dirs.
Require native/WASI byte identity and unchanged resource/error results before
timing. Preserve exact source and binary bindings for the disassembly and any
separate experiment; reject any byte mismatch rather than allowing tolerance.

Reject before timing if the pinned profile/code gives no meaningful F32 output
cost to remove. If tested, retain both-mode protected controls and every latency
or RSS miss. A successful arithmetic check alone does not justify the change.

## Source pins read while drafting

Base root: `/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura`.
The patch is against the retained N16 interval source, without temporary probes.

- `crates/pagura-render/src/compositor/narrow.rs`:
  `09e82dc8424dd53c3f45fe1533bc6a403311c52701b62436df103a977615967a`
- `crates/pagura-render/src/compositor/narrow_tests.rs`:
  `cb4c9f03b204bff27bfdd31ce24b6faef28a8c57c7dae2a33157f676a863ccfe`
- `docs/plans/f32-output-quantization-research.md`:
  `8283c665ed1856803b719d7a99909b0d207cdb33e17deae82a8ae2c7790e4537`

These pins identify the drafting base, not a validated build or a clean commit.
