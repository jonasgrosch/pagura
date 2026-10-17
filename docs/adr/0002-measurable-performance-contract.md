# ADR 0002: Performance claims require complete, correct comparisons

Status: accepted, 2026-10-17.

"At least 20% faster in all directions" becomes a release gate over a declared,
versioned matrix of operations, documents, output settings, engines and hardware.
It cannot establish superiority over every possible PDF or future implementation.

Use the stricter latency interpretation: Pagura time <= 0.80 * competitor time
(equivalent to at least 1.25x throughput at fixed concurrency). Enforce every
required comparison, not a geometric mean. Gate median and p95 with one-sided
95% bootstrap upper confidence bounds. Missing, failed, timed-out, unsupported,
incorrect or incomparable cells block the gate. Peak memory must not regress.

Correctness is checked outside timed regions for each measured result. Process
startup/end-to-end timing and in-process warm operation timing are separate tracks.
No comparison of a hot Pagura loop against a cold competitor executable. Native
and browser engines are identified with runtime and rendering backend versions.

Shared CI runs functional smoke tests only. Release evidence comes from a
controlled runner with randomized paired order, warmups, sufficient samples,
idle-machine notes, temperature/power constraints, binary/corpus hashes, and raw
measurements. Small bootstrap corpora are explicitly not release evidence.
