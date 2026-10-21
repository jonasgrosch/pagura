# F32 output profile after the retained N16 interval

The profile justifies an incremental F32 painted-interval experiment. It does
not measure a speedup, establish a memory benefit or satisfy an admission gate.
The retained product is `f9738174757252567730ff62b5d36620c01d1e6c`, with product
digest `53062678753e6012a291b95dbf07cb8b069e7134ab836d587919975d1ed057c7`.

One fresh native Exact profile used the independent quarter-alpha rectangle
reference on a 2048-by-2048 page. The existing example compiled once, rendered
one warmup and 1,000 reported iterations, and computed an image checksum outside
each render timer. The five-second sampler attached after 0.1 seconds. Both
owned processes completed, all 1,000 diagnostic checksums matched, and the
renderer was still alive when sampling ended. Build and profile receipts report
unchanged source, successful exits and no known-job contention. Polling cannot
exclude jobs shorter than its five-second interval.

Of 4,174 whole-process samples, 1,062 were inside the named render call chain.
Within that chain, `Buffer::output` has 572 samples (53.86%), `Context::commands`
274 and `Context::page_surface` 179. These are sampled call-chain counts on one
workload, not predicted speedup percentages. The other samples include the
example's hashing and printing and must not be charged to renderer work.

The binary's F32 output loop uses vector `frinta`, lower/upper clamps, unsigned
conversion and lane packing. The prominently sampled offsets +356 and +468
fall within that loop. An initial disassembly lookup omitted the Mach-O symbol
prefix and returned no instructions; the second lookup used the full symbol.
The archived disassembly is from the same profiled binary, without a rebuild.

This supports skipping conversion outside F32's conservative painted range.
It preserves eager initialization, all source evaluation and compositing,
precision, charges and the dense output contract. Dynamic sources mark the
full buffer once per operation, so images and shadings do not pay tracking cost
per pixel. The existing N16 and proposed F32 stores share a plain `Storage<T>`.
The [implementation plan](../../docs/plans/f32-painted-interval.md) describes the
opaque-background invariant and required gates. A separate incremental matrix
must protect dynamic sources, N16 and F64 and verify both policies and WASM.

The alternative F32 quantizer remains unapplied: it could help dense conversion,
but interval tracking removes entire conversion ranges. N16 row-loop trimming
also remains unapplied. No quantitative benefit is assigned to either idea.

## Reproduction and bindings

The [frozen plan](sources/f32-post-interval-profile/profile-plan.json),
[driver](sources/f32-post-interval-profile/profile.py),
[raw stack](sources/f32-post-interval-profile/sample.txt),
[record](sources/f32-post-interval-profile/record.json) and
[disassembly](sources/f32-post-interval-profile/output-disassembly-002.txt)
are archived. The plan retains the original local paths; archived copies have
identical bytes. Raw local evidence is under
`benchmarks/results/local/f32-post-interval-profile/`.

| Artifact | SHA-256 |
|---|---|
| Profile plan | `c10d62b637a6c751d673d1d9663edc0716a8cc35f067d8186697da88061aed6c` |
| Binary | `86b1c3f1b95c51d3b302d6f890bd3de75875b885963cf87d427da285640b857b` |
| Final build receipt | `db547bfc4278fb9a6a091603545a224a9ed913281d2773a1926421d067bdd07f` |
| Final profile receipt | `6557b9433248b94f7c17e074f309f013517035470728e69e9a97ea6e0bc52160` |
| Profile observer | `384ac5991bc3c81d7a78b24353946ef8eb6523329a06b644fb54266309f05cad` |
| Raw stack | `a07d1a39dfc363f5d8199016adae81d49411cbff21be963e1fbce715bf707fe2` |
| Disassembly | `506126e1c030cd99f13a5ed308f75bc97f958c7f9028e52358d50f76e58a5fbf` |
| `llvm-objdump` | `bb7793802ccffe8e0613ce326f6fe8caacbd626e5318bbcf73e820a5c027aca4` |

The disassembly command used CommandLineTools `llvm-objdump` with
`--disassemble-symbols=__RNvMs0_NtNtCs6mGY1dcbO7G_13pagura_render10compositor6narrowNtB5_6Buffer6output`
and `--no-show-raw-insn` on the plan's binary. F32 admission is the previously
validated expected route; this uninstrumented profile supplies no new route
counter. FNV is a diagnostic checksum, not independent byte verification of
every profiled image. The profile says nothing about WASM cost or competitors.
