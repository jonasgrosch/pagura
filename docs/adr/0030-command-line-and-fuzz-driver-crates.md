# ADR 0030: The command-line tool and the fuzz drivers are their own crates

Status: accepted, 2026-10-18.

## Context

The `pagura` package held both the public facade (`src/lib.rs`) and the
command-line tool (`src/main.rs`). A review found that this blurred what the
library ships and hid gaps in the facade:

- the tool imported `pagura_data::Bundled`, `pagura_crypto` and
  `pagura_document` directly, so nothing showed whether the facade was
  enough for a real caller;
- the library depended on `pagura-crypto` only for the tool, and the
  profiling commands (`microbench`, `scan`, `--timings`) were part of the
  library's package.

The fuzz drivers lived in `fuzz/src/`, a separate Cargo workspace for the
nightly libFuzzer binaries. The stable replay test in `crates/pagura/tests`
compiled them through `#[path = "../../../fuzz/src/lib.rs"]`, which is why
the facade had dev-dependencies on four engine crates it does not use, and
why the drivers were linted only as a module of that test.

## Decision

1. **`crates/pagura-cli`** builds the `pagura` binary. Its `bundled-data`
   (default) and `cjk-fonts` features forward to the facade's. Its aim is to
   depend on the `pagura` facade alone; each direct import of an engine
   crate marks something the facade does not expose yet.
2. **`crates/pagura-fuzz`** holds the fuzz drivers as a workspace member
   (test tooling, like `pagura-testkit`; never a dependency of a product
   crate). Its own test replays `fuzz/regressions/`. The `fuzz/` workspace
   keeps only the one-line libFuzzer binaries, now the `pagura-fuzz-targets`
   package, which call the drivers through a path dependency.
3. **Tools build the binary by name** (`cargo build --bin pagura`), which
   selects the right package both before and after this split, so the
   benchmark harness can still build historical revisions.

## Alternatives considered

- **Keep the binary in `pagura` behind a `cli` feature.** This keeps one
  package but still lets the binary reach the facade's private
  dependencies, and the library's feature set would mix product and
  tooling.
- **Keep the drivers in `fuzz/` and include them by path.** This keeps the
  fuzz workspace self-contained but leaves the drivers outside the
  workspace lints and keeps the facade's unused dev-dependencies.

## Consequences

- `cargo run -p pagura-cli -- …` or `--bin pagura` runs the tool;
  `-p pagura` builds only the library.
- The facade's tests are one test crate (`crates/pagura/tests/api/`), and
  the drivers' replay is `cargo test -p pagura-fuzz --test regressions`.
- Output file names of `convert` stay one-based (`page-0001.ppm` is page 0),
  matching pdftoppm and the benchmark harness, while every page argument
  stays zero-based.
