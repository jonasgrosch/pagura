# Retention review

Retain the painted-interval change on the experimental branch. Do not promote
it to main, deployed/default use or release on this evidence.

The complete confirmation verifies 21,320 outputs and both target latency
gates. Median render time decreases by 29.38% in Exact and 37.04% in Competitive.
The [full result](n16-painted-interval-experiment.md) preserves every cell and
the [machine-readable record](n16-painted-interval-experiment.json) includes
both arms' sampled RSS distributions.

Full admission fails: 42 of 50 protected latency cells and 24 of 52 sampled
RSS cells miss their bounds. Two protected cold medians have lower bounds
above the **1.02 material-regression threshold**: +3.50% Competitive and +4.46%
Exact. That classification does not assert that all other changes are neutral:
some other lower bounds are above 1.00 but below 1.02. Interpret references to
supported regressions in the generated result using this declared 2% threshold.

The Competitive target's observed maximum RSS increases by 9.60%; the largest
RSS miss is 32.29% in a protected control. Unchanged layouts and accounting do
not establish unchanged process RSS or explain those misses. Restrictions do
not remove the regressions from the implementation.

The measurement and audit completed with unchanged source and no known-job
contention observed. Bootstrap bounds come from the pinned harness; they were
not independently recomputed. The original interrupted run remains wholly
excluded. The next candidate needs its own evidence and cannot inherit a speed
or memory admission from this partial result.
