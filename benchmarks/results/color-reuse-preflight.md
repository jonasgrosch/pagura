# Color component reuse: accuracy preflight blocks timing

The allocation candidate preserves all 38 before/after RGBA pairs, but the
fixed independent gate passes only 74 of 76 outputs. Both arms fail
`paint/text-knockout-false` in Competitive mode: 192 channel values are 63
instead of the independently expected 64. Every output has the correct
dimensions and opaque alpha. Exact passes all 19 cases; Competitive passes 18.
These are analytic fixtures at 72 DPI, not a full-corpus accuracy score.

The result is a preexisting mode discrepancy, not an observed regression from
reusing the color component vector. Two coincident half-opacity red glyphs
on white should leave each green/blue component at 255/4 = 63.75, rounded to
64 once. Competitive downgrades the admitted F32 plan to U8, quantizes alpha
to 128, and stores the two results as 127 then 63. The retained-list image
cases all pass; this does not test the separate CLI direct-image path.

No speed or memory gain is established. The frozen zero-error gate was not
relaxed, no cells were removed, and neither timing pilot nor confirmation was
launched. Preserve the candidate in
[`candidate.patch`](sources/color-component-reuse/candidate.patch), repair
the composition discrepancy separately, then rebase it and freeze a fresh
incremental comparison. The production allocation change is not active in
this commit. This is preservation of an unmeasured idea, not rejection of a
measured partial win.

The candidate replaces a newly allocated component vector with validation
followed by clear/extend into existing storage. Seven new state/color tests
cover atomic errors, defaults, source positions, cloned state, finite numeric
conversion and storage reuse. The focused native run passed all ten matching
tests. Full-check-001 passed fixture/policy/format stages, then failed a new
test's empty-diagnostics assertion lint; the archived candidate fixes that
assertion, but its full check remains pending. Eight Python harness/lifecycle
tests pass. Escaped process-lifecycle errors abort the preflight; ordinary
renderer or pixel failures remain recorded across the entire matrix.

Both native artifacts were built serially and selected again from Cargo's
fresh executable records; the hashes match the original builds. The
preflight's runner receipt remains **failed, exit 1**, as required by the
quality result. Its source fingerprint was unchanged and the strict observer
found no known foreign compute in three scans. This is polling evidence,
not OS-level exclusivity. An earlier preflight invocation stopped before any
renderer call because a coordinator binding assertion failed; that failed
attempt is retained and contributed no samples.

See [`color-reuse-preflight.json`](color-reuse-preflight.json), the frozen
[plan](color-reuse-plan.json), [bindings](color-reuse-bindings.json), and
[recheck script](sources/color-component-reuse/summarize_preflight.py).
Raw receipts, all 76 RGBA outputs, PDFs and oracles remain under
`benchmarks/results/local/color-component-reuse/`. The recheck validates
artifact hashes, exact commands, independent pixels and all paired bytes;
the source-to-binary relationship trusts the recorded coordinator build.
