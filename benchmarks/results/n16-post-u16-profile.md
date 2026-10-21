# Remaining work after the exact u16 converter

2026-10-21. One fresh profile of committed `28c3234c`, whose product fingerprint
matches the independently validated u16 candidate. This is hypothesis selection,
not a comparative speed measurement or admission result.

The existing Exact `render_profile` example opened/compiled opaque64 once and
rendered 1,001 times, reporting the final 1,000 diagnostic FNV checksums. They all
match the independently byte-verified confirmation output. One owned sampler
requested five seconds at one-millisecond intervals. Both children completed;
cleanup, artifact hashes and the final runner receipt were verified. Seven
observer scans found no known competing jobs. The renderer was still alive when
the sampler returned after 5.13 seconds; that wait is not a precise capture length.

The whole-process sample includes 3,257 self samples in the example's main
function, consistent with its inlined full-image checksum loop outside the
render timer. Do not count those as renderer work. The identified render subtree
contains 540 samples:

| Call chain | Samples |
|---|---:|
| Command execution | 274 |
| Inlined output-loop-dominated aggregate | 171 |
| Page-surface creation | 70 |
| Output Pixmap initialization | 22 |
| Context cleanup | 3 |

Disassembly of this exact binary maps the aggregate's listed `render_policy`
offsets 9408/9404 to addresses `0x1001bb6c0/0x1001bb6bc`, within the N16
`uqadd`/`ushr`/`subhn` vector output loop. The sampled addresses have the same
ASLR slide. The profiler collapses additional offsets into an ellipsis, so the
aggregate is output-loop-dominated rather than an instruction-perfect allocation.
These counts locate remaining work; they are not calibrated stage timings.

**Next: conservatively skip untouched N16 page output.** The fixed fixture has
one 256×6 strip in each 256×256 chunk. A flat painted interval would analytically
retain 98,304 of 4,194,304 conversions, or 2.34375%, assuming the stated integer
rectangle mask extents. Full output initialization, N16 initialization, command
execution and rasterization remain. No speed or memory reduction follows without
the next separately frozen experiment.

Keep dense `Buffer::output` unchanged; skip only where page output already holds
the same opaque background. Track both writers after successful validation and
preserve resource/error behavior. Measure native/WASM enclosing layouts and
protect dense fills, distant marks, tiny/cold cases, other formats, colored
backgrounds, full/offset tiles and recycled storage. Reject or revise if added
bookkeeping, layout, accuracy or protected costs defeat the measured benefit.

Preparing source words once per path is deferred. Only 21 named `Buffer::path`
samples appear in this workload; some are actual blending rather than source
encoding. It cannot remove the 31 `Runs::next` or 11 source-conversion samples
elsewhere in the painting chain. Absence of a symbol is not absence of inlined work.

[Bound profile evidence](n16-post-u16-profile.json) and
[executed source archive](sources/n16-post-u16-profile/README.md) preserve the
fixed plan, binary/source/build identities, raw sample/checksum hashes and limits.
No baseline profile or performance samples are pooled into this diagnostic.
