# ADR 0019: JavaScript development tooling is pinned, locked and never shipped

Status: accepted, 2026-10-18. Applies the development-tooling rules of ADR
0018 to the JavaScript; the viewer's architecture (ADR 0005) is unchanged.

## Context

About 3,000 lines of JavaScript surround the engine: the browser viewer in
`web/*.mjs`, plain ES modules with no npm package, bundler, CDN or build
step; the Node harness `tools/host_test`, which checks native/WASI parity,
the browser worker shim and the viewer against independent oracles; the
real-Chrome check `web/browser_test`; and three exploratory benchmarks in
`web/`. Nothing type-checked this code, the harness was one `main()` of
`node:assert` calls, and no formatter or linter ran.

Three alternatives were rejected. A Rust/WebAssembly UI framework would add
a build step and binary size to DOM glue that is small and has no
performance problem. TypeScript compiled to JavaScript would end the
buildless viewer: the browser would no longer load the committed files.
Deno or Bun would replace Node, but the harness relies on `node:wasi` and
`worker_threads`, and CI pins Node.

## Decision

1. **The viewer stays buildless JavaScript.** The browser loads `web/*.mjs`
   exactly as committed. Types are JSDoc comments, and `tsc` checks them
   with `checkJs` and `strict`. `web/tsconfig.json` checks the shipped
   modules against browser globals only, without Node types. Typedefs are
   shared through type-only `@import` tags, so no type exists at run time
   and no module is added. A cast states in a comment the invariant that
   the checker cannot see.
2. **Node-only scripts are TypeScript run directly by Node.** `host_test`,
   `browser_test` and the benchmarks are `.ts` files that Node 26 runs by
   stripping types; nothing is compiled or emitted. `erasableSyntaxOnly` and
   `verbatimModuleSyntax` restrict them to syntax that stripping removes.
   In `web/`, `.mjs` marks a shipped module and `.ts` a Node-only script.
3. **The harness uses `node:test`.** `tools/host_test.ts` has named
   `describe`/`test` cases with shared `before`/`after` hooks. A run can
   select cases with `--test-name-pattern`, and the reporter names each
   case. It still imports only Node built-ins and the viewer's own modules.
4. **Development only, pinned and locked.** A private root `package.json`
   lists TypeScript, Biome and the Node type declarations as exact
   `devDependencies`. `package-lock.json` locks every package, including
   each platform's prebuilt binary, by integrity hash. `.npmrc` keeps later
   installs exact and runs no install scripts. `npm ci` installs the tools
   once, like a toolchain. `tools/check.py` then runs them from
   `node_modules` without network access. No product crate and no viewer
   module imports them, and the development server serves `web/` only.
   Updates are deliberate commits that change the lock and fix what the new
   version reports.
5. **Biome formats and lints.** One package with a prebuilt binary per
   platform and no JavaScript dependencies replaces Prettier and ESLint,
   which with typescript-eslint lock 96 packages and need two
   configurations. It formats with two-space indentation, 120 columns and
   single quotes. Import sorting is off because import order is module
   evaluation order. Linting uses the recommended preset except
   `noNonNullAssertion`: the Node-only scripts use `!` where a test has
   already established presence. A suppression comment names its rule and
   reason. Two paths are excluded. `benchmarks/adapters/` has its own
   package and its sources are part of the recorded benchmark engine
   identities. A generated `web/protocol_constants.mjs` keeps the layout its
   generator writes; `tsc` still checks it.

## Consequences

Contributors run `npm ci` once, with network access, before
`tools/check.py`, and need Node 26.8.1 or newer. New viewer code carries
JSDoc types and new Node scripts carry TypeScript annotations, under strict
checking. Type checks do not establish browser behaviour: the DOM harness
still does not cover real-browser layout, selection or touch, and
`browser_test` remains a manual real-Chrome check. The benchmark adapters
stay unformatted until a deliberate change re-records the benchmark
identities.
