# Temporary F32 benchmark route check

Unexecuted, candidate-only source diagnostic. This patch adds one test module and
four `cfg(test)` API-entry hooks. It changes no renderer fields, release code,
arithmetic, budget charges, dispatch decisions or production fixtures. The test
uses the actual compiled commands and actual raster/cache policy for the first
page chunk. It is not performance evidence or a replacement for the independent
full-page preflight.

Base: candidate worktree at retained HEAD
`f9738174757252567730ff62b5d36620c01d1e6c`, including its current uncommitted F32
interval product. Exact pre-patch Git blobs:

* `crates/pagura-render/src/compositor.rs`: `11cd68c675c49c02483a51a06e977f28cb167287`
* `crates/pagura-render/src/compositor/narrow.rs`: `07aa8d1df7b64cfe8d696debf32fbaf6acd57da1`

The patch contains no `narrow_tests.rs` changes and retains the explicit
`Self::U8(_) => None` match. Apply the patch, never copy entire overlays.

## Fixed requests and independently derived expectations

Six frozen benchmark PDFs, both Exact and Competitive: exactly 12 first chunks.
All requests are 72 DPI, full page canonical transform, default Repair opening,
normal full page compilation, opaque white background. The output destination is
the actual first chunk's extent. The fixture dimensions, root command classes,
selected page storage and chunk side are asserted before rendering.

| Case | Exact / Competitive storage | Side | Narrow interval | Narrow output calls / pixels |
| --- | --- | --- | --- | --- |
| sparse64 | F32 / F32 | 256 | Exact 59904..61440; Competitive 59904..61696 | 6 / 1536; 7 / 1792 |
| opaque64 | N16 / N16 | 256 | same as sparse64 | same as sparse64 |
| image-only | F32 / F32 | 128 | 0..16384 | 128 / 16384 |
| f32-shading-dense | F32 / F32 | 256 | 0..65536 | 256 / 65536 |
| f32-pattern-dense | F64 / F32 | 256 | None / 0..65536 | 0 / 0; 256 / 65536 |
| default-knockout64 | F64 / F64 | 256 | None | 0 / 0 |

The sparse/opaque glyph run at PDF y1808..1814 maps to device y234..240,
width 256. The Competitive fragment mask submits its additional zero phase row,
so its conservative union is seven rows. Dynamic F32 consumers submit every
first-chunk pixel. Exact uses path spans, Competitive uses fragment mask calls,
and the respective glyph cache has retained data after rendering.

`default-knockout64` is the wide nested-group control: retained text knockout
forces F64. The similarly named `nested64` has simple isolated groups and is
admitted F32, so it is deliberately not mislabeled as a wide control here.

Every first-chunk RGBA pixel is independently asserted. The glyph stripe is
black for opaque64 and RGB191 for quarter-alpha sparse/knockout text; its other
pixels remain white. The three quarter-alpha dynamic fills have RGB191
throughout. Alpha is always 255. This uses literal fixture geometry/source-over
expectations, not another Pagura render. Output conversion must leave the
already-consumed work/allocation/function counters unchanged.

Counters record narrow API entries, not successful/nonzero stores. F64 controls
require a real wide surface, zero narrow writer/output entries, and the same
independent pixel check; zero narrow output counts do not mean wide conversion
was skipped. No scheduler/full-page chunk-count claim is made.

## Coordinator execution and binding

Root alone runs compilation/tests. Create a directory **inside the candidate
root** containing exactly the six frozen preflight PDFs named `<case>.pdf`.
Externally retain an input manifest with their absolute paths, byte counts and
SHA256 hashes; verify those hashes before and after the run. This test does not
implement its own hashing. Pin the patch, test module, manifest, Cargo command,
environment, module/binary, runtime and final runner receipt in existing evidence.

Set `PAGURA_F32_ROUTE_INPUTS` to that absolute directory at **Cargo build time**.
The test accepts a runtime value first, then the compiled `option_env!` value.
The latter permits the existing WASI wrapper's `env: {}` with its exact candidate
root preopen; no wrapper change or hardcoded checkout path is needed. Pin either
runtime override too and reject an unexpected override. The compile-time value
must resolve inside the preopened root. Native focused test filter:

```text
cargo test --release -p pagura-render --lib compositor::temporary_f32_routes::temporary_f32_benchmark_routes -- --exact --nocapture --test-threads=1
```

Use the coordinator's existing serial build flags, strict guard and receipt.
If also run on WASI, use the existing wrapper and exactly the same filter and
arguments; archive and hash the actual executed module. The probe emits exactly
12 `PAGURA_F32_ROUTE_V1` lines on success. Preserve any failed assertion/output.
No unit test, compilation or rendering has been performed by the author.

## Restore before measuring

Capture the full product source digest before installation and verify the two
base blobs above. Refuse installation if either differs. Apply only the owned
patch after `git apply --check`; do not format or modify instrumented files.
After the test, use `git apply --reverse --check` and reverse that exact patch.
Verify both original blobs, absence of `temporary_f32_routes.rs`, and the exact
original full product source digest. Stop on restoration mismatch; do not reset
or overwrite unrelated edits. Never benchmark an instrumented test binary or
retain these hooks in the product.
