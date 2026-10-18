"""Offline product checks, identical locally and in CI after toolchain installation."""
import sys

# Before anything else: the tools need tomllib (3.11); .python-version pins the exact version.
if sys.version_info < (3, 11):
    sys.exit(f"tools/check.py needs Python 3.11 or newer, not {sys.version.split()[0]}; "
             "run it with `uv run tools/check.py` to use the version pinned in .python-version")

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARGO = ["cargo"]
OFFLINE = ["--locked", "--offline"]
# Python lint tools pinned in uv.lock (ADR 0018); `uv sync` installs them once.
UV_LINT = ["uv", "run", *OFFLINE, "--only-group", "lint"]
# CONTRIBUTING convention 13: crates whose features change what compiles are
# also linted and tested with none and with all of their features, since the
# workspace runs unify the defaults back on. Features select code, not
# platform behaviour, so only Linux runs them; each set rebuilds the graph.
FEATURE_MATRIX = [
    (CARGO + [step, *OFFLINE, "-p", crate, features, "--all-targets", *tail], {})
    for crate in ("pagura", "pagura-cli", "pagura-data")
    for features in ("--no-default-features", "--all-features")
    for step, tail in (("clippy", ["--", "-D", "warnings"]), ("test", []))
] if sys.platform.startswith("linux") else []
COMMANDS = [
    ([sys.executable, "tools/policy.py"], {}),
    ([sys.executable, "tools/fixtures.py", "--check"], {}),
    ([sys.executable, "tools/bundled_data.py", "verify"], {}),
    (UV_LINT + ["ruff", "check"], {}),
    (UV_LINT + ["basedpyright"], {}),
    (CARGO + ["fmt", "--all", "--", "--check"], {}),
    (CARGO + ["clippy", *OFFLINE, "--workspace", "--all-targets", "--", "-D", "warnings"], {}),
    (CARGO + ["test", *OFFLINE, "--workspace", "--all-targets"], {}),
    # The host as shipped (no bundled data, ADR 0005) and with every opt-in
    # feature, each on its own: a workspace build unifies bundled-data on.
    (CARGO + ["clippy", *OFFLINE, "-p", "pagura-host", "--all-targets", "--no-default-features", "--", "-D", "warnings"], {}),
    (CARGO + ["clippy", *OFFLINE, "-p", "pagura-host", "--all-targets", "--all-features", "--", "-D", "warnings"], {}),
    (CARGO + ["test", *OFFLINE, "-p", "pagura-host", "--all-targets", "--no-default-features"], {}),
    (CARGO + ["test", *OFFLINE, "-p", "pagura-host", "--all-targets", "--all-features"], {}),
    (CARGO + ["test", *OFFLINE, "--workspace", "--doc"], {}),
    (CARGO + ["doc", *OFFLINE, "--workspace", "--no-deps"], {"RUSTDOCFLAGS": "-D warnings"}),
    *FEATURE_MATRIX,
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--target", "wasm32-wasip1"], {}),
    # Matching opt-in font builds exercise informational diagnostics without
    # changing the default WASI host's data footprint.
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target-dir", "target/host-bundled"], {}),
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target", "wasm32-wasip1", "--target-dir", "target/host-bundled"], {}),
    # Native/WASI parity, the browser shim and the viewer harness (Node, no npm packages).
    (["node", "tools/isolation_test.mjs"], {}),
    (["node", "tools/viewer_navigation_test.mjs"], {}),
    (["node", "tools/tile_cache_test.mjs"], {}),
    (["node", "tools/host_test.mjs"], {}),
    (["node", "tools/editing_host_test.mjs", "target/host-bundled/wasm32-wasip1/release/pagura-host.wasm"], {}),
    ([sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"], {}),
    ([sys.executable, "tools/conformance.py"], {}),
    # Every table generator in tools/gen/ verifies its output (CONTRIBUTING, Conventions).
    *(([sys.executable, path.relative_to(ROOT).as_posix(), "--check"], {})
      for path in sorted((ROOT / "tools/gen").glob("*.py"))),
]

if __name__ == "__main__":
    for command, extra_env in COMMANDS:
        print("Checking: " + " ".join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True, env={**os.environ, **extra_env})
    print("All checks passed. This is not a complete PDF conformance or performance certification.")
