# Browser viewer

Build the pinned WASI command and serve the dependency-free viewer:

```
cargo wasm --offline
python3 tools/serve_web.py
```

Open http://localhost:8000 in a browser with WebAssembly SIMD, module threads,
SharedArrayBuffer and cross-origin isolation. There is no npm package, bundler,
CDN or build step for the HTML/CSS/JavaScript. Do not open index.html as a file
URL: the server provides the required COOP/COEP headers.

Select a PDF or drop it onto the viewer. Pages scroll continuously. Metadata
and pixels load as pages approach the visible region; offscreen canvases and
page elements are discarded. Previous/next and numbered navigation scroll to a
page without changing documents. Unimplemented content produces a tagged error
on the affected page and clears partial pixels for that page. Other pages can
still render. Strict validation rejects repairs and names the refused repair;
normal viewing lists applied repairs below the document.

## Controls

- Zoom: fit width, fit page, percentage presets, or Ctrl/Command-wheel. Two-finger
  touch pinch also changes document zoom. Fit presets use the selected page's
  physical dimensions; changing the selected page or resizing recalculates them.
- Keyboard: Page Up/Down select adjacent pages, Home/End select the first/last,
  +/− change zoom and 0 restores 100%. Editing fields and buttons keep their
  normal keyboard behavior. Zoom is limited to 10%–800%. Native arrow-key scrolling works in the focusable
  document region.
- Password-protected PDFs open an accessible modal with retry, Cancel and Escape.
  The field is cleared when submitted, cancelled or replaced by another file;
  temporary password bytes are cleared after each open attempt. Text entry sends
  UTF-8 bytes. Non-ASCII legacy passwords requiring PDFDocEncoding are not
  converted by the UI; callers can use SessionClient.open with explicit raw
  bytes. ISO 32000-2 7.6.4.3.2 and 7.6.4.3.3 distinguish those encodings.
- Controls have labels and standard focus order. Pages have named image roles,
  loading states and explicit failures. This is visual navigation accessibility,
  not tagged-PDF text extraction or a screen-reader text representation.

The layout uses page geometry from ISO 32000-2 7.7.3.3 (Table 31) and 8.3.2.3.
Compact prefix sums retain discovered heights without creating a DOM element
for every page. Detailed metadata is capped at 256 pages, visible page elements
at 32, and canvases at 128 tiles. Tiles are 256 device pixels, painted at
zoom × devicePixelRatio. Layout extents are limited to 10 million CSS pixels;
exceeding a budget displays an error rather than silently truncating a document.

## Session and caching

The browser negotiates protocol v1.1; existing protocol v1 clients remain
supported. See crates/pagura-host/PROTOCOL.md for the byte layouts. A viewport
change invalidates queued old-generation tiles before transmission. The host
also skips a stale generation before compilation or painting. The current
synchronous tile cannot be interrupted; its result is discarded if obsolete.
Opening another file replaces the instance and releases its caches.

Compiled display lists are cached per document/page, independently of zoom or
device pixels. The session keeps an LRU with a default 64 MiB accounting budget
(or Limits.decoded_bytes, if smaller) and at most eight entries. Close releases
all entries for that document. Lists exceeding the accounting budget render
without retention. Session::with_cache_bytes(0) disables retention.

Accounting charges 32 source bytes per input byte and 4096 bytes per painted
mark, plus a fixed 512-byte header charge. These are estimates: DisplayList
exposes no retained allocation size. The weight budget is not a hard process-memory ceiling;
expanded paths/resources may differ from the estimate. Per-operation renderer
limits remain in force. A renderer allocation-size API would allow accurate
retention accounting. Keeping the cache in the session preserves separation
between source documents and rendering state. Caching pixels instead would
consume more memory and require additional keys for zoom and output settings.

The project-written WASI shim exposes no preopened directories, sockets,
environment or arguments. Only the dedicated execution thread blocks on
Atomics.wait; stdin is a 64 KiB shared mailbox and stdout uses transferred byte
chunks. Unknown WASI imports trap by name. PDF interpretation receives no URL
or file capabilities. A user-selected file stays local to the browser.

## Development fixtures

```
python3 tools/serve_web.py --fixtures
```

Then open:

```
http://localhost:8000/?file=/fixtures/classic/two-pages.pdf
```

The development-only flag additionally serves corpus/generated read-only at
/fixtures/, on loopback only, with the same isolation and path protections.
Without the flag those URLs return HTTP 404. There are no upload/write routes
or directory listings; private/reference corpora are never exposed.

Startup loading accepts one same-origin /fixtures/ URL. Other origins or paths,
parent traversal, credentials, nested query/fragment and duplicate file
parameters are rejected before fetching. Redirects are refused, credentials
are omitted, and received bytes are capped at 512 MiB. Picking/dropping another
file takes precedence over a pending startup load.

Automation can wait for #status = "Visible tiles rendered" and #error hidden,
then inspect .tile canvases inside section[data-page="0"] or another zero-based
page section. Both pages of two-pages.pdf coexist when visible. Its second page
is white except for the blue square at device x=32..63, y=0..31 at scale 1.
text-unsupported.pdf exercises the failure banner; trailing-bytes.pdf exercises
repair display. Existing real-browser verification covered isolation, exact
DPR-2 pixels, navigation and the explicit failure banner. The continuous layout,
new gestures and modal also have offline DOM tests and need browser interaction
checks on the target devices.

## Verification and exploratory profiling

```
cargo build --locked --offline -p pagura-host
cargo wasm --offline
node tools/host_test.mjs
```

Tests preserve complete native/WASI response identity and independent classic
PPM oracles, plus page-2 full/partial geometry oracles, generation skips, cache
reuse/eviction, continuous lazy layout, navigation, fit presets, gestures,
password retry/cancel, diagnostics and explicit failures. In-memory HTTP tests
check isolation headers, MIME types and path containment without a socket bind.
The DOM harness does not establish real-browser layout or touch behavior.

Build release binaries for each revision and run the same workload against each:

```
cargo build --locked --offline --release -p pagura-host
cargo wasm --offline
node web/tile_benchmark.mjs target/release/pagura-host target/wasm32-wasip1/release/pagura-host.wasm > /tmp/tiles.json
cargo run --locked --offline --release -p pagura-host --example tile_profile -- corpus/generated/classic/many-rectangles.pdf
```

The benchmark emits binary/input/output hashes, hardware/runtime identities and
raw seconds/tiles-per-second samples. It uses two warmups and five alternating
samples per runtime for 64 fixed tiles of many-rectangles.pdf, verifying every
pixel against its independent PPM after timing. Native samples include process
startup; WASI samples include _start/open and exclude module instantiation.
Compare revisions within each runtime, not native and WASI timings to each
other. The profile separately measures 64 compilations and 64 cached-list tile
paints. These are exploratory diagnostics, not the sample count, confidence
intervals, memory measurements or controlled runner required by the release
policy in benchmarks/policy.json. They make no cross-engine speed claim.
