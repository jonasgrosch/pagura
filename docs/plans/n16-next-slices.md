# Next measured slices on the shared compositor

These are candidates, not implemented or measured gains. Complete and commit
the frozen guarded N16 conversion experiment before changing its product
sources. Keep the glyph memo and earlier clip work in both comparison arms.
The standing accuracy, resource and competitor goals remain unchanged.
Re-profile the retained candidate before selecting the later structural slice;
an earlier hot spot may shrink after an intervening optimization.

## 1. Keep the exact converter in 16-bit lanes

Replace the current widened `n = u32::from(word) + 128` with
`n = word.saturating_add(128)`, retaining `(n - (n >> 8)) >> 8`.
For words through 65,407 the arithmetic is unchanged. Above that boundary,
the original rounded division and the saturated expression both return 255.
The subtraction cannot underflow. The existing exhaustive u16 and destination
offset/tail tests must pass, followed by native/WASM validation.

The hypothesis is that narrower arithmetic reduces vector widening and
packing. Generated instructions and measured latency must establish it;
source-level simplicity does not prove faster code. There is no storage or
work-accounting change. Use the same 46 cells and gates against the committed
guarded converter, with fresh fixed pilot and confirmation seeds. Reject if
the target benefit does not reproduce or protected regressions outweigh it.

## 2. Skip untouched N16 output

Track one conservative painted interval per N16 chunk. The two write paths
are `Buffer::path` and the fragment-mask word sink. Extend the interval only
after destination/clip validation and immediately before an infallible store;
mark whole submitted spans, including zero coverage. Avoid per-pixel scans,
additional allocations and per-row metadata in the first experiment.

Leave `Buffer::output` semantics unchanged for arbitrary destinations. At the
page-output boundary, intersect each output row with the painted interval.
Skipping other pixels is sound only when N16 admission requires an opaque
background and final output already contains the same background. The current
background guard is therefore a correctness prerequisite.

Tests must cover empty chunks, mixed masks, distant writes, full fills,
nonzero tile origins, multiple chunks, clipping errors after a successful
row, and unchanged resource failures. Verify `Buffer`, `Surface` and mask-sink
sizes on native and WASM; two extra words in one enum variant do not by
themselves prove unchanged enclosing layouts. Freeze sparse, dense,
distant-mark and full-fill cases before timing, with U8/F32/F64 controls.
Reject if bookkeeping costs defeat sparse gains, layout grows materially,
or soundness requires broader tracking machinery. Allocation, initialization
and scratch reuse are separate experiments.

## 3. Resolve immutable glyph paint once per command

Preflight currently resolves required sources before culling, then execution
can resolve the same Exact glyph paint once per visible glyph. Competitive
fragment rendering already resolves it once per run visit. Start by omitting
discarded preflight conversion only for direct DeviceGray/RGB/CMYK into RGB,
where conversion is total and consumes no function work. Preserve traversal,
command/depth charges, `/None` behavior and stroke validation.

Then separate solid-color resolution from coverage application. A function-free
glyph run can resolve lazily at its first existing resolution point and reuse
that floating source only within the same run invocation. Preserve the first
conversion's position after geometry, clipping and their charges. Scope the
first version to RGB without separation, spot or mask-color passes. No scene
hash key, eviction policy or retained Context allocation is required.

ICC/CIE/Indexed reuse needs explicit dependency classification. Potentially
failing sources still require preflight for off-region diagnostics. Tint and
other function-bearing sources remain unchanged: charging one cached total
cannot reproduce intermediate budget checks, failing positions or partial
work consumption. Transfers remain deferred device operations.

Freeze many-glyph device-color targets with Competitive fragments, ICC,
single-glyph, paths, images and tiny cold controls. ICC becomes a target only
in a later revision that explicitly admits its reuse. Reject changes to required
off-region ICC/Type4 errors, stroke validation, function/raster budget
boundaries, strict precision or colored-background output. Fewer conversions
without measured benefit do not justify more code.

All slices use separate source/binary bindings and fresh samples. Retain useful
partial wins experimentally with every failed gate visible. Do not add gains
from different workloads, treat baseline pixels as accuracy truth, or call
these candidates evidence of release or worldwide superiority.

After the individually measured slices settle, freeze a separate net-stack
comparison against the earlier combined renderer. Separate byte-equivalent
cases from independently adjudicated accuracy corrections. The opaque legacy
route's incorrect partial-coverage output cannot serve as a correctness gate;
its timing can still expose the cost paid to fix that output. Only a direct
same-workload experiment can establish how the retained gains stack together.
