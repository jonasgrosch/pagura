# Preparation profile: many small marks

2026-10-21. One fixed five-second sampling capture of `microbench render-cold`
completed with the unchanged release binary independently audited in the
[19-case competitor pilot](analytic19-conversion.md). The 1,000-iteration child
and sampler exited successfully, both owned groups were cleaned up, and the
runner source and artifact bindings passed. Six observer scans found no known
competing job. This is explanatory profile evidence, not a speed comparison.

The fixture paints 16,384 separate one-pixel rectangles. Its pending numeric
operand vector never exceeds four values and already reuses its capacity.
The three disjoint children of the sampled `render_page` branch are:

| Work | Samples / 3,649 | Sample share |
| --- | ---: | ---: |
| Compile page | 1,856 | 50.9% |
| Render display list | 1,325 | 36.3% |
| Destroy display list | 468 | 12.8% |

These counts describe this call-tree branch, not all process samples or an
elapsed-time decomposition. Raw anchors are lines 69–73, 341–343 and 537–653 of
`local/many-marks-profile/run-001/sample.txt`. The entire raw profile and logs
are retained and hashed in the [result bindings](many-marks-profile.json).

`Color::set` has 89 inclusive samples: 46 freeing its replaced component vector,
21 allocating the replacement and 22 other samples. This justifies measuring a
small reuse change, while setting a modest expectation: it does not eliminate
the separate default-color allocation, retained color clones, paths or list
destruction. No percentage speedup is inferred from these samples.

Other candidates have stronger observed reach. Rectangle construction's close
and move-to branches contain vector growth; five separately appended segments
can grow a fresh path vector twice. The allocator's `empty_fill::Edge` symbol
does not establish empty-fill geometry work: merged/inlined monomorphizations
limit that interpretation. Integer-reference lookahead appears in 217 parser
samples, but removing its validation would change malformed-input behavior.

Rendering also spends 131 samples reserving/rehashing the retained hull cache,
37 inserting and 22 hashing lookups, versus 25 in the sibling hull computation.
Cache eligibility changes need particular care: later hits affect bounds-work
charges. These findings motivate separate proposals with unchanged validation,
identity safety, work limits and output contracts.

The immediate sequence is to measure color-buffer reuse, then evaluate rectangle
allocation and hull-cache overhead. The N16 painted-interval proposal remains
separate and useful for its larger-pixel workload; this profile does not evaluate
that workload. All comparative gains still require independent output checks,
fixed paired timing and memory evidence.

[Frozen protocol and source review](sources/many-marks-profile/README.md),
[driver](sources/many-marks-profile/profile.py), and
[pre-execution pins](sources/many-marks-profile/plan.json) record the scope.
The reference campaign independently verified 42 outputs for this binary and
input; the microbench black-boxes its results and does not verify each iteration.
