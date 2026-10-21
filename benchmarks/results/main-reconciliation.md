# Main reconciliation of the experimental renderer

2026-10-21. The integration merges main `d81d0c82` into experimental parent
`237f134a`. It preserves the experimental history rather than rebasing measured
commits. The old common ancestor was `752b52e7`. Existing benchmark source,
binary and sample identities remain unchanged; this integration makes no new
speed, memory or release claim.

## Resolution

The JPEG decoder now includes main's loaded-bit invariant and its equivalent
optional sparse-coefficient constructor. The opaque image sampler restores
main's `c19c5374` source-line footprint order, boundary cache and scratch/work
accounting. Its production code matches main exactly; three additional tests
cover independently expected pixels on irregular tiles, shared-boundary work
limits, error positions and scratch admission/release in both option sets.

The prior sampler reorder changed four historical Exact rounding-tie cells.
Those original input/settings bindings were not reconstructed here. Restoring
main is compatibility and resource reconciliation, not proof of universal
accuracy improvement or an unchanged work-limit threshold. The tests retain
their declared tie rules; strict fixture bounds were not relaxed.

The branch retains its N16/F32 composition, F64 background guard, source-error
checks, glyph storage limits, exact output conversion, glyph memo and clip
work. Main's image-expansion cache remains present, but normal N16/F32 planning
does not necessarily execute the byte-page optimization. Inclusion of the code
does not establish that main's measured byte-page speed carries over.

The content/paint manifests, fixture generation and conformance checker stay
byte-identical to the first parent. A clean automatic merge had reintroduced
four looser sliver tolerances; those were removed during review. The branch's
independent composition fixes keep their ordinary strict bounds.

## Validation

The full serial repository check passed in 857.13 seconds: formatting, lint,
workspace tests/docs, native/WASI host checks, 196 Python tests and all 1,249
conformance cases. The native suites include 142 raster and 229 renderer tests.
The five focused sampler tests also passed in release builds on native,
WASI SIMD and WASI scalar. Scalar explicitly disables `simd128` and uses a
separate target directory. Executed WASI module bytes are hash-bound in logs.

The separate 24-output native/SIMD/scalar image check passed strict RGB,
opaque-alpha and same-mode cross-platform identity checks. Its fixed scope is
four independent image references, both modes, page 0 at 72 DPI on white.
It uses full compilation and retained-list rendering;
specialized direct-image dispatch and drawing-only validation are not inferred.

All execution is serial under the shared cooperative runner and five-second
known-process observer. These controls can miss short or unrecognized jobs;
they do not establish a dedicated release runner. The
[validation sources](sources/main-reconciliation/README.md) and
[verified evidence record](main-reconciliation.json) retain the command,
source, module, reference, output and receipt checks. This is a finite regression
matrix, not a general PDF conformance or cross-platform perfection claim.

## Next measurement

The [direct start-to-current plan](../../docs/plans/start-to-current-measurement.md)
takes priority over the next optimization. Session baseline `359e19d` has the
same product/build sources as available control `676a872`; its retained profile
driver is byte-identical to the current example. A new bound comparison must
measure the combined stack directly, including the costs of corrected output.
Earlier clip, glyph and output-conversion percentages are incremental and are
not a cumulative speedup. The parked rectangle candidate stays absent.
