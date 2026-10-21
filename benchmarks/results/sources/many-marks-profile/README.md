# Many-operands preparation diagnostic — source review, unexecuted

Source root: `/Users/jonasgrosch/.codex/worktrees/renderer-stacked-gains/pagura`.
Baseline reported by coordinator: c70b4678. Re-pin the actual CLI binary,
complete source identity and final build receipt before execution. No build,
test, import, profile or renderer was executed for this review.

## What the case really exercises

`tools/fixturegen/classic.py:50–59` builds 128×128 separately painted 1×1
rectangles. Each is `R 0 B rg x y 1 1 re f`, alternating red/blue. This means
16,384 retained Path commands, 114,688 numeric tokens and 49,152 operators;
the numeric operand vector is never longer than four. The independent oracle
is the exact 128×128 checkerboard at 72 DPI, with tolerance zero. It is a
many-small-marks and preparation workload, not a huge pending operand array.
The many-rectangles control (`classic.py:36–44`) has 1,024 separate 16×16 marks
on 512×512 pixels: sixteen times fewer marks, sixteen times more pixels.

The frozen campaign manifest at
`benchmarks/results/sources/analytic19-conversion/manifest.json:149–164` pins:

- PDF SHA256 `be3e7f89248060bd7ed07c885dd4aafd453db0ce99dfaf0881b9f20a93fb50c9`.
- PPM SHA256 `78250fff9aafca7d0babfdcd43fd5b389e55d55facd4d4d54316f28e270baf82`.

Coordinator-supplied, independently output-matching exploratory results were
Pagura/Hayro engine times 17.513/7.377 ms for many-operands and 1.299/0.906 ms
for many-rectangles; many-operands process RSS ratio was 2.894. These motivate
profiling, but do not attribute the difference to a particular stage or allocation.

## Smallest useful existing profile

Use the existing CLI, without a new product instrument or precompiled-list loop:

```
PINNED_PAGURA microbench PINNED_MANY_OPERANDS_PDF render-cold 1000 72
```

Default mode is Exact; record the actual flags and runtime defaults. The CLI
loop in `crates/pagura/src/main.rs:533–569` reopens the in-memory input and calls
`render_page` every iteration for `render-cold`. Thus it includes open/page
preparation, content compilation, rendering, and resulting object destruction.
The file is read before the loop; this is not a cold-filesystem or startup test.
It constructs one unused warm context before the loop, also outside its internal
timer. It does not emit or independently verify the repeated images.

Launch that child through the already-reviewed owned-profile coordinator.
After a fixed 0.1-second attachment delay, run exactly one:

```
/usr/bin/sample OWNED_PID 5 1 -mayDie -file FRESH_OUTPUT/sample.txt
```

Keep the fixed 1,000 iterations, five-second capture and 1 ms sampling period;
do not increase/retry after seeing stacks. Require child and sampler success,
the expected microbench operation/iteration count, no observed contention,
and unchanged source/input/binary pins. If the child ends before useful capture,
report insufficient profile evidence. Profile sampling is whole-process and
includes allocations/deallocation; neither sample shares nor the instrumented
elapsed time constitute a speed comparison.

Before sampling, one ordinary `render --timings` of these same frozen bytes
must pass the existing retained-output audit against the independent PPM.
The completed campaign output can satisfy this prerequisite only when its
binary and settings are exactly the profiled ones. Do not claim the
microbench verified every iteration: it uses black_box, not byte verification.
Retain argv, environment, source/binary/input/oracle hashes, final build/run/
observer receipts, raw stdout/stderr and full sampling report.

If the first capture requires one prespecified diagnostic control, run
many-operands `render-warm 1000 72` separately after it, with the same five-second
sampling window. This still recompiles via `RenderContext::render_page` each
iteration (`context.rs:174–208`); it retains document/font/image resources.
It distinguishes repeated open/cache effects, not retained-display-list rendering.
Do not subtract two profile sample counts to estimate compilation time.
`render_profile.rs:15–22` compiles once before its loop and is unsuitable for
this preparation question. No compile-only CLI operation currently exists.

## Two concrete hypotheses, not measured attribution

1. **Small color allocations per mark.** `interpreter/state.rs:66–91` replaces
   the fill Color for every `rg`; `color.rs:61–73` allocates default components,
   and `color.rs:103–117` collects another Vec while setting them. The retained
   PathCommand clones Color (`interpreter/path.rs:118–129`), including its Vec.
   Consequently the source requests multiple short component-vector allocations
   per rectangle, plus the retained vector in each command. The optimized build
   may eliminate some temporary work; the profile must establish actual reach.
   Look for Color::new/set/clone, Vec allocation and deallocation under compiler
   paths. DeviceRGB's temporary Name is inline (`pagura-core/src/name.rs:8–38`),
   so do not misattribute that name to a heap allocation.

2. **Per-path geometry and retained command footprint.** `path.rs:54–76` in the
   interpreter appends five segments for each rectangle. Raster PathBuilder::finish
   (`pagura-raster/src/path.rs:180–189`) transfers the vector with mem::take,
   leaving the next mark to grow a new segment vector. Painting also allocates
   Arc<Path> and retains a command (`interpreter/path.rs:88–129`). This is a
   plausible contributor to allocator time and peak retained memory. Sample the
   construction/finish/Arc/list-growth and drop stacks separately from raster
   work; RSS alone cannot distinguish them. A later allocation-count diagnostic
   would be necessary for byte/count attribution; do not infer counts from
   sampling shares or claim all RSS difference is geometry.

Counterchecks: `interpreter.rs:441–450` takes then restores and clears the same
operand Vec, so there is no source-level allocation per numeric operand.
Numeric values are inline (`pagura-core/src/value.rs:35–43`). The number parser
already has a short-integer path, but speculatively scans a reference tail for
unsigned integers (`syntax.rs:680–778`); repeated token scanning is a possible
CPU cost, not an established allocation cause. Rectangle fill certification
already has an allocation-free sufficient path (`empty_fill.rs:18–31`), so
do not blame uncertain-geometry flattening/sorting without seeing those frames.
Content assembly also copies decoded stream bytes once per compile
(`interpreter.rs:298–316`), a secondary candidate if memcpy dominates.

## Small follow-up only after the profile

If color allocation is visible, first consider eliminating the throwaway
default-components allocation when constructing a validated solid color, or
reusing component capacity with a separate full-validation pass before writes.
Do not introduce an inline-container dependency or change retained Color layout
until the smallest change has evidence. If retained path allocations dominate,
investigate a bounded rectangle representation separately; it is a larger
geometry/API change and is not justified merely by this synthetic case.

Every proposed change must preserve DefaultRGB substitution and validation,
numeric/arity checks, successful state transitions and failure order, compound
operand behavior and illegal-reference errors, object/stream offset context,
token/32-operand/path/display/fill-work limits, and existing allocation failure
behavior. Parser specialization must still reject indirect references and
retain error positions; simply turning reference parsing off changes semantics.
Never merge differently colored marks or skip preparation/source validation to
win this case. Recheck exact checkerboard bytes plus existing color-default,
malformed-content, split-stream, compound-operand and resource-boundary tests.
Any eventual speed claim needs a fresh source-bound paired preparation/engine
comparison and protected controls, separate from this explanatory profile.
