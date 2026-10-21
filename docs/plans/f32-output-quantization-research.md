# F32 output quantization: source-only candidate

This is a follow-on hypothesis, not an implemented change or measured gain.
Keep it separate from the N16 painted-interval experiment. Inspect a pinned
F32 output profile and generated native/WASM code before allocating another
performance experiment. The current converter already uses f32 arithmetic;
there is no f64 promotion to remove from this output loop.

The current expression is `value.round().clamp(0.0, 255.0) as u8`.
An alternative is `(value + f32::from_bits(0x3eff_ffff)) as u8`, using the exact
constant `0.5 - 2^-25`. Unlike interval tracking, it changes no storage,
initialization, traversal, blending, allocation or work accounting.

Plain addition of 0.5 is incorrect: the f32 predecessor of 0.5 rounds up to
1 when added to 0.5, although its current byte output is 0. The proposed
constant addresses that boundary rather than permitting a one-byte difference.

For each byte transition n=1..255, let t=n-0.5 and p be the preceding f32.
Under round-to-nearest, ties-to-even f32 addition, t plus the proposed constant
rounds to n, while p plus the constant rounds to the f32 immediately below n.
At n=1 the first equality deliberately uses a tie at 1; at larger transitions
the subtracted 2^-25 is less than half the spacing. Monotonic addition and
saturating truncation then establish the complete finite byte intervals.
Negative inputs and negative infinity saturate to 0; positive overflow and
infinity saturate to 255; NaNs cast to 0, as with the existing expression.
This source-level equivalence argument still requires implementation checks.

Before timing, independently test predecessor/exact/successor at all 255
transitions with expected n-1/n/n, every integer byte, signed zeros, signed
subnormals, finite extremes, infinities and varied quiet/signaling NaNs. Exercise
the actual dense F32 output loop with distinct lanes, nonzero source offsets,
partial destination pixels and overlong destinations. Keep the existing
source-half-level and accumulation tests. Verify actual native, WASI SIMD and
WASI scalar output identity; do not use the old converter as the only oracle.

Native vector rounding may already be inexpensive. Reject the idea if the
pinned code/profile gives no meaningful reason to expect a gain, or if any
output, work/error or resource contract changes. If it earns a separate frozen
experiment, retain the existing both-mode protected controls and disclose all
incremental latency/RSS misses. No speed percentage follows from this proof.
