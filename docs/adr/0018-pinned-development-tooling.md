# ADR 0018: Development tooling is pinned, locked and never product

Status: accepted, 2026-10-18. Complements ADR 0008, which covers product
crates only.

## Context

About 19,000 lines of Python check, generate fixtures for and benchmark
Pagura: `tools/check.py` and its policy checks, the independent fixture
generators and oracles in `tools/fixturegen/`, the benchmark harness in
`tools/bench.py`, `tools/benchkit/` and `benchmarks/`, and the fuzz seeds.
Rust, Node and the CI actions were pinned exactly; Python was whatever the
runner image shipped, the benchmark engines were pinned only at the top
level in `benchmarks/requirements.txt`, and nothing type-checked or linted
the Python.

Python stays the tooling language. The fixture oracles must be independent
of the Rust engine they check (ADR 0006), and the harness drives competitor
engines through their Python bindings.

## Decision

Tools that never ship may use third-party packages under these rules:

1. **Development only.** No such package becomes a dependency of a product
   crate, and none is imported by product code. Fixture generators stay
   standard-library only; checkers analyse them but are never imported by
   them.
2. **Pinned and locked.** `.python-version` pins the interpreter and
   `pyproject.toml` the minimum the tools support (3.11). Each tool is an
   exact version in a dependency group; `uv.lock` pins every transitive
   package by hash (ADR 0002). The pin is the newest 3.13 release, not 3.14:
   the official Windows builds of 3.14 implement `zlib` with zlib-ng, whose
   deflate output differs from zlib's, and the generated Flate fixtures are
   compared byte for byte on every platform. CI installs uv by exact version and every
   action by commit. Updates are deliberate commits that change the lock and
   fix what the new version finds.
3. **Offline checks.** `uv sync` installs the tools once, like a Rust
   toolchain; `tools/check.py` then runs them with `uv run --locked
   --offline`, so a check never touches the network or an unlocked version.
4. **Isolated competitors.** The `bench` group (PDFium and MuPDF bindings,
   Pillow) is installed only into `$PAGURA_COMPETITORS_DIR/venv` by
   `tools/competitors.py`, never into the repository's `.venv` (ADR 0006).
5. **Licences.** Checkers and package managers need a permissive licence.
   Competitor engines are run as benchmark subjects, never linked into or
   copied into Pagura.

The tools are:

- **uv** manages the interpreter, environments and the lock.
- **ruff** lints for defects: pyflakes (`F`), syntax errors (`E9`), bugbear
  (`B`) and pylint errors (`PLE`). It does not enforce style. B023 is ignored
  because the closures it flags run inside the iteration that defines them,
  B905 because `zip(strict=...)` is a per-call decision, and B007 because loop
  variables may be named for the data they skip. `ruff format` is not adopted: it would
  rewrite 61 of the 62 Python files, including the compact one-line style of
  the fixture generators. Adopting it is a separate, repository-wide change.
- **pyright** type-checks in standard mode, which infers types in
  unannotated code; mypy `--strict` instead requires annotations everywhere
  and reports over 5,000 errors in `tools/` alone. It is installed as
  **basedpyright**, which ships pyright's checker and its Node runtime as
  wheels, so `uv.lock` pins it and it runs offline. The `pyright` package on
  PyPI downloads pyright from npm on first use, outside the lock. The
  configuration stays in `[tool.pyright]`, which editors running pyright or
  Pylance also read. The Python oracles under `crates/` are linted but not
  yet type-checked.

## Consequences

New Python must pass ruff and pyright. A `# pyright: ignore[rule]` names its
rule and is reserved for deliberately malformed fixture data and for flow the
checker cannot follow. Contributors need uv in addition to the Rust
toolchain and Node; `uv run tools/check.py` uses exactly the interpreter and
tools CI uses.
