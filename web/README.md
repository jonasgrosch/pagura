# Browser viewer

From the repository root, with the pinned toolchain installed:

```
cargo wasm
python3 tools/serve_web.py
```

Open http://localhost:8000 in a browser supporting core WebAssembly SIMD,
module workers, SharedArrayBuffer, and cross-origin isolation. No npm packages,
bundler, CDN, installation or network refresh is involved. The server serves
only web/ and the built command, with COOP/COEP; do not open index.html as a
file URL. Select/drop corpus/generated/classic/rectangle.pdf, navigate pages
and change zoom. The file stays in the browser. Strict validation can be
selected before opening; otherwise reported repairs are shown below the page.
Unsupported pages display an explicit tagged error and remove partial tiles.
Strict-mode errors also name the refused repair, for example
`recovery would repair it [file.trailing-bytes]`; that repair was not applied.
The interim foundation renderer deliberately rejects many PDFs and fractional
edges at some zoom/devicePixelRatio values; this is displayed as Unsupported.

For automated real-browser checks, opt in to **development fixtures**:

```
cargo wasm --offline
python3 tools/serve_web.py --fixtures
```

Then open:

```
http://localhost:8000/?file=/fixtures/classic/two-pages.pdf
```

The development-only `--fixtures` flag additionally serves files beneath
`corpus/generated/` read-only at `/fixtures/`, with the same isolation headers,
traversal rejection and symlink containment as web/. It stays loopback-only;
there is no upload or write endpoint, no directory listing, and private/reference
corpora are never mapped. Without the flag, fixture URLs return HTTP 404. The
normal picker/drop viewer continues to work without it.

The `file` parameter loads one same-origin `/fixtures/` URL at startup. A root
path or an absolute URL with the viewer's exact origin is accepted. Other
origins, other paths, traversal (including encoded parents/backslashes),
credentials, queries/fragments inside the file URL, and repeated `file`
parameters are rejected with a visible error before fetching. Redirects are
refused, credentials are omitted and received bytes are capped at 512 MiB.
This URL comes from the viewer location, never from interpreted PDF content.
Picking/dropping a file during the startup fetch takes precedence.

Automation can wait for `#status` to read `Visible tiles rendered`, then check
`#error` is hidden, inspect `.tile` canvases and drive `#next`, `#previous` or
`#page`. The two-page fixture has a red 16×16 square at PDF (8, 8) on page 1;
page 2 has a blue 32×32 square at PDF (32, 32). At scale 1, page 2's top-left
pixel coordinates x=32..63, y=0..31 are blue and all other pixels are white.
Use `?file=/fixtures/classic/text-unsupported.pdf` to check a visible tagged
failure and `?file=/fixtures/classic/trailing-bytes.pdf` to check repair display.

Tiles are 256 device pixels and only the visible region is requested at
zoom × devicePixelRatio. Offscreen canvases are discarded. The viewer caps
visible tiles at 128 and layout extents at 10 million device pixels; the host
has independent Limits. Opening another file replaces the worker, releasing
its document caches and pending work. Page/zoom generations discard stale
results. The protocol also supports multiple documents in a single session.

The worker owns a project-written preview 1 shim (wasi.mjs). Only its thread
blocks on Atomics.wait. Main-thread stdin uses a 64 KiB shared mailbox with
atomic ownership and asynchronous drain notifications; responses arrive as
transferred byte chunks. No preopened directories, network APIs, environment
or arguments are exposed to the module. Unknown WASI imports trap by name.
The worker itself fetches only the fixed built module URL; PDF bytes cannot
influence that URL. Synchronous rendering has no in-request cancellation;
destroying/replacing the worker releases the session.

Verification (no downloads):

```
cargo build --locked --offline -p pagura-host
cargo wasm --offline
node tools/host_test.mjs
```

The test runs all classic render cases natively, under node:wasi with empty
args/env/preopens, and through the actual browser shim/client in a Node worker.
It checks complete response byte identity and independently generated PPM pixels,
plus malformed framing, structured errors, tiled assembly and capability denials.
It also exercises viewer.mjs with a minimal DOM/Canvas harness, including all
classic oracles, tile eviction on scroll, rapid zoom changes, DPR changes, drop,
repair display, refused repair tags and clearing tiles on unsupported input.
Both page_info results and page-2 full/partial tiles of two-pages.pdf are
checked against independently computed geometry. The DOM harness tests
previous/next/numbered navigation and startup fixture loading/rejection. In-memory HTTP handler
tests verify headers, MIME types and path containment without opening a socket.
These checks do not claim actual browser layout or browser interaction coverage.

A real-browser check (served with `--fixtures`, opened at
`/?file=/fixtures/classic/two-pages.pdf`) confirmed cross-origin isolation,
pixel-exact tiles at device pixel ratio 2, previous/next navigation and the
explicit unsupported-content banner. File picker and drag-and-drop are not
automated. The
viewer has no password prompt; callers can send raw password bytes through
SessionClient.
