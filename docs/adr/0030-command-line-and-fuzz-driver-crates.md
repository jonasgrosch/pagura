# ADR 0030: The command-line tool and the fuzz drivers are their own crates; the facade is complete

Status: accepted, 2026-10-18.

## Context

The `pagura` package held both the public facade (`src/lib.rs`) and the
command-line tool (`src/main.rs`). A review found that this hid gaps in the
facade and blurred what the library ships:

- the tool imported `pagura_data::Bundled`, `pagura_crypto` and
  `pagura_document` directly, because the facade offered no shareable font
  provider, no complete licence notices and no way to name the types a
  `Provider` returns or `TextPage` takes;
- the library depended on `pagura-crypto` only for the tool, and the
  profiling commands (`microbench`, `scan`, `--timings`) were part of the
  library's package;
- the facade's "public API" tests imported `pagura_text`, `pagura_render`,
  `pagura_data` and `pagura_core`, so they could not show that the facade
  was enough;
- `pagura::compile_page_with_text(document, page, None)` silently rendered
  without the bundled fonts that `pagura::compile_page` used, because one
  entry point was re-exported unwrapped beside wrapped ones.

The fuzz drivers lived in `fuzz/src/`, a separate Cargo workspace for the
nightly libFuzzer binaries. The stable replay test in `crates/pagura/tests`
compiled them through `#[path = "../../../fuzz/src/lib.rs"]`, which is why
the facade had dev-dependencies on four engine crates it does not use, and
why the drivers were linted only as a module of that test.

## Decision

1. **`crates/pagura-cli`** builds the `pagura` binary and depends on the
   `pagura` facade alone. Its `bundled-data` and `cjk-fonts` features
   forward to the facade's; both are defaults (amended 2026-10-27, ADR 0009),
   while the facade's own default stays `bundled-data`. Arguments are parsed into a typed
   command before any input is read; usage errors exit with status 2 and
   run-time failures with 1; commands write to any `io::Write`.
2. **The facade is complete and deliberate.** The root holds documents,
   pages, the object model, rendering and writing, the free functions and
   the types their signatures name. Domain modules hold the rest:
   `pagura::text` (the extracted-text model), `pagura::provider` (the
   `Provider` trait, the types it returns, the bundled provider and
   `default_provider`), and the existing `forms`, `form_appearance`, `navigation` and
   `signatures`. Every free function uses the feature-selected provider;
   an explicit provider goes through `render_context_with_provider` or
   `RenderContext::with_provider`. A test imports nothing but `pagura` to
   implement a provider, lay out and search text and fill a form. `notices()` lists every notice a build must reproduce, including
   the always-linked Unicode data.
3. **`crates/pagura-fuzz`** holds the fuzz drivers as a workspace member
   (test tooling, like `pagura-testkit`; never a dependency of a product
   crate). Its own test replays `fuzz/regressions/`. The `fuzz/` workspace
   keeps only the one-line libFuzzer binaries, now the `pagura-fuzz-targets`
   package, which call the drivers through a path dependency.
4. **Tools build the binary by name** (`cargo build --bin pagura`), which
   selects the right package both before and after this split, so the
   benchmark harness can still build historical revisions.

## Alternatives considered

- **Keep the binary in `pagura` behind a `cli` feature.** This keeps one
  package but still lets the binary reach the facade's private
  dependencies, so it would not force the facade to be complete, and the
  library's feature set would mix product and tooling.
- **Re-export individual items of `forms` and `navigation` instead of the
  modules.** Those modules expose only their public API, so a list in the
  facade would duplicate it and drift whenever `pagura-document` adds a
  type; the completeness test catches a missing type instead.
- **Keep the drivers in `fuzz/` and include them by path.** This keeps the
  fuzz workspace self-contained but leaves the drivers outside the
  workspace lints and keeps the facade's unused dev-dependencies.

## Consequences

- `cargo run -p pagura-cli -- …` or `--bin pagura` runs the tool;
  `-p pagura` builds only the library.
- The facade's direct dependencies grow by `pagura-text` and
  `pagura-font`, which it already linked through `pagura-render`; no new
  crate enters the graph.
- The facade's tests are one test crate (`crates/pagura/tests/api/`), and
  the drivers' replay is `cargo test -p pagura-fuzz --test regressions`.
- A facade-wide `pagura::tags`, which re-exports every crate's tag
  modules, waits until each crate has `features` and `repairs` with `ALL`
  (ADR 0017); it comes with a workspace test of tag uniqueness.
- Output file names of `convert` stay one-based (`page-0001.ppm` is page 0),
  matching pdftoppm and the benchmark harness, while every page argument
  stays zero-based; the help text states both.
