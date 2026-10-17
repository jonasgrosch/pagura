# ADR 0005: WebAssembly is the primary target, hosted as a WASI command

Status: accepted, 2026-10-17. WebAssembly in the browser is the main target.

## Context

Every Rust target forbids unsafe code. In Rust 2024, exporting functions from
a WebAssembly module requires `#[unsafe(no_mangle)]` or `export_name`, which
`forbid(unsafe_code)` rejects. `wasm-bindgen` is a third-party code generator
that emits unsafe glue. A desktop window needs FFI. None of these fit.

## Decision

- The engine ships as `pagura-host`, a std-only **WASI command** built for
  `wasm32-wasip1`. It serves the Pagura session protocol over standard input
  and output. No exports beyond `_start` are needed, so the product stays free
  of unsafe code. The same binary builds natively for tests and desktop shells.
- `wasm32-wasip1` is chosen over `wasm32-wasip2` because browsers run core
  modules directly and Node ships a preview 1 implementation (`node:wasi`) for
  CI; components need extra transpilation tooling. Revisit when browsers or
  WASI 0.3 change that. SIMD128 is enabled for autovectorization only.
- **Capabilities**: the host grants no preopened directories, no sockets, no
  environment and no arguments. Interpretation therefore cannot touch the
  filesystem or network, enforced by the runtime, not just by policy.
- **Browser**: a Web Worker instantiates the module with a project-owned,
  dependency-free WASI shim (`web/`) implementing only `fd_read` (blocking on
  a `SharedArrayBuffer` with `Atomics.wait`), `fd_write`, `clock_time_get`,
  `random_get` (unused by the engine), `proc_exit`, `args_*`/`environ_*`
  (empty) and `fd_prestat_get` (no preopens). This needs cross-origin
  isolation (COOP/COEP headers). The UI is plain HTML/JS with no npm packages.

## Protocol v1

Little-endian binary frames: `u32 length`, `u8 kind`, `u32 request id`,
payload. Requests: `hello(version)`, `open(bytes, recovery, password)`,
`page_info(doc, page)`, `render_tile(doc, page, scale, x, y, width, height)`,
`diagnostics(doc)`, `close(doc)`. Responses mirror requests; failures are
`error` frames with `ErrorKind`, feature or repair tag, message, object and
position — the structured `pagura_core::Error`, never free text alone.
Frames are bounded by `Limits`; oversized or malformed frames end the session
with an error frame. Tiles are returned as straight RGBA8 for `ImageData`.

## Consequences

One long-lived instance holds open documents and their caches. Each request
is synchronous; cancellation and parallel tiles come with threads later.
The protocol is also how native shells and benchmarks can drive a warm engine.
