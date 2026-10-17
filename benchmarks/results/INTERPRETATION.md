# Initial benchmark findings

**The 20% target is not achieved.** The harness is working and preserves this
failure instead of declaring success from favorable averages or wrapper overhead.

The [six-engine smoke run](bootstrap.md) contains 42 measured comparison cells:
seven original analytic fixtures against PDFium, MuPDF, PDF.js, Poppler, hayro
and Apple CoreGraphics. Every one of its 840 timed outputs matched the independent
pixel oracle. Adobe remains unavailable. These fixtures exercise only the
implemented opaque rectangle subset, not representative PDF rendering.

The [focused hayro run](hayro-native.md) uses 100 paired samples and five warmups
per fixture. All 1,400 timed outputs matched. Pagura's observed median end-to-end
latency was roughly 7–15% lower across these cases. Every case failed the required
upper-confidence-bound test for at least 20% lower latency. Raw samples, median
and p95 ratios/bounds, and memory measurements are in the raw report referenced by [its summary](hayro-native.md).

Both native programs incur process startup, file I/O and PPM output. Python/Node
adapters incur their runtime/import overhead as well. The large differences
against those adapters must not be described as core rendering speedups. This
was a shared macOS desktop, not an attested performance runner.

Next: implement equal-work in-process benchmark tracks, expand correctness and
feature coverage, bind representative release workloads, and profile measured
bottlenecks. Full ISO conformance, Adobe comparison, editing workloads, multiple
resolutions/concurrency levels and cross-platform evidence remain open. A faster
partial page or an omitted comparison can never satisfy the release contract.
