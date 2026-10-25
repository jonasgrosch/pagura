# ADR 0035: Band threads on one prepared scene

Status: proposed, 2026-10-25. Refines ADR 0034 §9 and the render budgets of
ADR 0016 (amendment of 2026-10-21); follows ADR 0008 for dependencies.

## Context

The renderer executes bands of the global device grid, and preparation is
its only fallible step (ADR 0034 §4, §7). ADR 0034 §9 noted that
`PreparedScene` is `Sync` and bands are independent, so band threads could
follow; the roadmap lists native band threads on `std::thread::scope`.

The single-thread scoreboard (ADR 0002, ADR 0007) is not affected: it
measures one thread against one thread, and its process accounting flags any
engine above 1.05 CPU-seconds per wall-second. No competitor exceeded that in
the round 5 runs (700 samples each). Of the engines compared:

- hayro 0.8 sets `num_threads: 0` on vello_cpu, with vello_cpu's
  `multithreading` feature off;
- MuPDF renders bands of a display list on worker threads in
  `mutool draw -T threads -B band_height`, with one cloned context per
  worker;
- PDFium is not thread-safe;
- pdf.js paints on the main thread.

So threads are a product and wall-time question, measured on a track of
their own.

Until 2026-10-18 the renderer had coverage-lookahead threads. An ablation
measured 2.7× wall time on one file for 7% more CPU, and nothing for
WebAssembly or under the scoreboard's CPU budget, so they were removed. The
note at the time put parallelism, if wanted, at the tile or page level in the
host, with an ADR, since ADR 0004 already names tiles as the unit. This record
compares that option with band threads on one prepared scene.

A prototype (this change) renders the bands of one `PreparedScene` on
several threads. A host-tile driver renders horizontal stripes on the band
grid on several threads, each stripe prepared on its own as a host would
tile it today. Both ran on the 80 benchmark jobs: 40 pages at 72 and 150 dpi.
The machine was an M3 Pro (6 performance and 6 efficiency cores), so a
render on 8 threads runs at least 2 of them on efficiency cores. Other
sessions shared it at a one-minute load average of 7–15 (median 10), and
runs held the bench lock.

Medians ran in one process per job, with variants in rotating order over 9
rounds. Ratios are geometric means over jobs. A second full run, at a median
load of 8.4, agreed within 0.06 on every ratio below. These are exploratory
numbers on a shared machine, not a performance claim (ADR 0002).

- A cold render opens the document, records the page, prepares it and
  executes it in fresh storage.
- A warm render prepares and executes a recorded scene in kept storage.
- Thread counts include the calling thread.

**Band threads** — wall-time speedup over one thread, and CPU time
(user plus system) relative to one thread:

| Threads | 2 | 4 | 8 |
|---|---:|---:|---:|
| Cold, wall | 1.26× | 1.44× | 1.46× |
| Warm, wall | 1.57× | 2.23× | 2.61× |
| Warm, execution alone | 1.75× | 2.84× | 3.51× |
| Cold, CPU | 1.05 | 1.12 | 1.32 |
| Warm, CPU | 1.08 | 1.18 | 1.46 |

Preparation stays serial, as do opening and recording, which bound the cold
gain. The share is one thread's median:

| Pages | Warm, 4 / 8 threads | Cold, 4 / 8 threads | Serial share, warm | Serial share, cold |
|---|---:|---:|---:|---:|
| Image | 3.20× / 4.40× | 1.66× / 1.80× | 0% | 47% |
| Text | 2.33× / 2.77× | 1.50× / 1.52× | 11% | 47% |
| Mixed | 1.97× / 2.20× | 1.41× / 1.41× | 21% | 53% |
| Vector | 1.69× / 1.74× | 1.24× / 1.19× | 35% | 63% |
| 72 dpi | 2.00× / 2.19× | 1.31× / 1.30× | | |
| 150 dpi | 2.49× / 3.11× | 1.59× / 1.65× | | |
| Small pages | 2.13× / 2.45× | 1.37× / 1.38× | | |
| Large pages | 2.42× / 2.94× | 1.44× / 1.43× | | |

- No warm render got slower at any thread count, including those of 0.3 ms.
- Each helper costs about 60 µs of CPU per render on the smallest page:
  spawning, restarting its storage, and the bands it sets up.
- Five cold renders were up to 10% slower on 8 threads.

**Host stripe tiles** on the same jobs and threads, each stripe prepared
separately:

| Threads | 2 | 4 | 8 |
|---|---:|---:|---:|
| Cold, wall | 1.20× | 1.30× | 1.20× |
| Warm, wall | 1.37× | 1.77× | 1.87× |
| Cold, CPU | 1.21 | 1.54 | 2.51 |
| Warm, CPU | 1.22 | 1.49 | 2.36 |

On cold image pages, 8 stripes spent 4.19× the CPU of one thread: every
stripe decoded the same image at once.

**Against single-thread hayro 0.8**, in the same session (wall ÷ hayro,
below 1 is faster):

| Threads | 1 | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| Cold | 1.66 | 1.31 | 1.15 | 1.13 |
| Warm | 1.19 | 0.76 | 0.54 | 0.46 |

**MuPDF 1.28.2** was measured without installing `mutool`. A driver
reproduces `mutool draw -T workers -B band_height` against the library
PyMuPDF bundles, and N counts workers besides the coordinating main thread.

| Workers | 2 | 4 | 8 |
|---|---:|---:|---:|
| `-B 256`, cold, wall | 1.26× | 1.43× | 1.50× |
| `-B 256`, warm, wall | 1.42× | 1.64× | 1.71× |
| `-B 256`, CPU cold / warm | 1.18 / 1.17 | 1.46 / 1.48 | 1.59 / 1.63 |
| `-B 32`, cold, wall | 1.30× | 1.51× | 1.67× |
| `-B 32`, warm, wall | 1.50× | 1.75× | 1.88× |
| `-B 32`, CPU cold / warm | 1.24 / 1.22 | 1.70 / 1.77 | 2.49 / 2.61 |

- MuPDF runs a display list once and renders bands of it on workers with
  cloned contexts. With a cold store and `-B 32`, one worker took 2.2× its
  own direct render: image areas are decoded again for each band.
- Pagura on N threads ÷ MuPDF `-B 256` on N workers is 1.49, 1.35, 1.10 and
  0.98 warm (N = 1, 2, 4, 8), and 0.58–0.60 cold. Pagura's N includes its
  calling thread.

**Memory.** A helper holds one scratch:

- a median of 1.07 MB at 72 dpi and 1.87 MB at 150 dpi, at most 6.7 MB on
  these pages;
- in peak resident memory, 1.0 MB and 1.8 MB per extra thread, at most about
  6 MB, against 14 MB and 22 MB for a one-thread render.

**Bytes.** Every variant was byte-identical on all 80 jobs. So were the
1,246 fixture renders at 1 and 8 threads (1,040 renders, and 206 errors
reported identically).

## Decision

### 1. Threads live in the band executor; the host chooses how many

`pagura-paint` renders the bands of one prepared scene on scoped threads
(`std::thread::scope`) for the duration of one render. It keeps no threads,
pool or thread state between renders. The caller chooses the count by lending
one `Scratch` per thread: `PreparedScene::with_scratches(&mut [Scratch])`.
`with_scratch` stays the one-thread form. A host (`pagura-render`'s
`RenderContext`, the CLI, a native viewer) turns its own setting into that
many scratches and keeps them between renders, as it keeps one today.

Host tiles remain what a viewer asks for (visible areas, ADR 0005). They are
not the way to spend threads on one region:

- Each tile repeats preparation for the marks it meets.
- Concurrent cold tiles build the same `Store` values at once (ADR 0012 lets
  two threads build one key).
- Budgets and errors depend on the region (ADR 0016 §4), so tiling a page for
  speed would change which limits it meets.

Band threads share one preparation, and its charges and error do not change.
Rendering several pages or documents at once stays the host's, with one
scratch per thread as now.

### 2. Output does not depend on the thread count

Every band executes as it does on one thread. Each band writes its own rows
of the output, a disjoint slice of the collected surface. A worker's active
lists advance over any gap between its bands, and its glyph rows kept for the
next band are used only when that band is next (ADR 0004). Bytes, the
`Completion` and the work counted are therefore those of one thread for any
number of threads. Tests render every executor test scene with three
helpers, and the busy scene at four band heights with one to four threads.
The benchmark jobs and fixtures above are byte-identical as well.

### 3. Budgets: helpers take only what preparation left

Preparation's reservations, charges and error are unchanged and do not depend
on the thread count. After preparation, a render admits as many helper
threads as `render_bytes` still has room for, at one scratch reservation each
(`Plan::scratch`). It admits no more than the region has bands beyond the
first, and no more than the caller lent scratches for. Each helper's scratch
fits within its own reservation, as the first one's does. A helper whose
storage cannot be allocated, or whose thread cannot start, is left out, and
the remaining threads take its bands. So:

- `render_bytes` still bounds what one render holds (ADR 0016, amendment of
  2026-10-21);
- a tight budget means fewer threads, never an error;
- `render_work` and `function_work` are spent once, split across threads; the
  per-thread usages sum to the one-thread usage (tested).

### 4. Hand-out and stopping

Bands go out in order from one locked queue. Each thread takes the next band
when it finishes one, holding the lock only to take it. `stop` is asked in
band order by whichever thread takes the band, so it must be `Send`. After a
stop, the rows of every band before it are rendered and the rest are
truncated, as on one thread.

`render_bands`, the streaming consumer, stays on the calling thread in this
decision. Making it parallel needs in-order delivery from out-of-order
bands: a bounded window of about two band buffers per thread, charged to
`decoded_bytes` as its one band is now, with the consumer called on the
calling thread. That waits for a host that needs it.

### 5. WebAssembly stays on one thread

WebAssembly builds admit no helpers (a compile-time constant). They allocate
nothing more and spawn nothing, and their behaviour and bytes do not change.
WebAssembly threads need shared memory, cross-origin isolation in browsers
and a host that spawns workers (ADR 0005). They need their own decision.

### 6. Dependencies: `std` only

The prototype uses `std::thread::scope`, `std::thread::Builder::spawn_scoped`
and `std::sync::Mutex`. rayon and its `crossbeam` dependencies contain
`unsafe` code, which ADR 0008 §2 excludes. A persistent pool is not needed
either. Spawning per render costs about 60 µs of CPU per helper, including
its storage restart (§ Context), and no render got slower warm.

## Consequences

- **Warm renders gain most.** A viewer that keeps recorded scenes renders
  2.2× faster on 4 threads and 2.6× on 8, and image pages up to 4.4×. Cold
  renders gain 1.4–1.5×, because opening, recording and preparing stay on one
  thread. Parallel preparation would be the next lever, if it is ever wanted.
- **The CPU cost is modest up to 4 threads:** 5–8% on 2 threads and 12–18% on
  4. Some of it is per helper (spawning and restarting storage). Some is the
  glyph rows a band no longer inherits from the band before. On 8 threads,
  efficiency cores and the machine's load take it to 32–46%.
- **Each thread costs a scratch:** about 1–2 MB at screen resolutions, within
  `render_bytes`.
- **API changes.**
  - `PreparedScene::with_scratches` is added; a `Rendering` holds its helpers.
  - The `stop` of `Rendering::render_until` and of `pagura-render`'s
    `render_until` becomes `Send`.
  - `render_bands` is unchanged and stays on one thread.
  - `pagura-render`, the CLI and native hosts get a thread setting and keep
    that many scratches in a follow-up. The default stays one thread.
- **The scoreboard does not change.** Thread results stay a separate wall-time
  track, always reported with CPU time and against MuPDF's band threads.

## Alternatives considered

- **Host tiles for speed** (the earlier note): stripes prepared separately on
  threads of their own. On the same threads they gained less wall time (warm
  1.87× against 2.61× on 8) for much more CPU (2.36–2.51× against
  1.32–1.46×). Every stripe repeats preparation for the marks it meets, cold
  stripes decode shared images at once, and the region changes the budgets.
- **A thread count on `RenderRequest`.** The request determines preparation
  and its charges, which must not depend on threads. The thread count is a
  property of the storage a render executes in, so it travels with the
  scratches.
- **Charging every helper at preparation** (`threads × scratch` against
  `render_bytes`). It would make charges and errors depend on the thread
  count; admitting helpers from the room left keeps them fixed.
- **rayon or another pool crate.** Excluded by ADR 0008 §2 (`unsafe` in
  rayon-core and crossbeam). `std::thread::scope` borrows the plan, shapes and
  output without `'static` bounds or reference counting.
- **A static split of contiguous band ranges per thread.** Within each
  range it would keep the glyph rows a band carries to the next (ADR 0004),
  which save about 3% on text pages on one thread. But pages often put
  their cost in a few bands (an image at the top, text below), and one range
  then sets the wall time.
  Dynamic hand-out balances, and loses only that carry.
- **Parallel preparation.** Preparation reserves budgets and reports the
  first error in source order. Splitting it across threads needs a different
  reservation scheme. It stays serial, and it bounds the cold gain (§
  Consequences).
- **WebAssembly threads now.** These need shared memory and cross-origin
  isolation (or JSPI), and the viewer's worker pool is a roadmap item of its
  own (ADR 0005). Deferred.
