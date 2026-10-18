"""Offline product checks, identical locally and in CI after toolchain installation."""
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARGO = ["cargo"]
OFFLINE = ["--locked", "--offline"]
COMMANDS = [
    ([sys.executable, "tools/policy.py"], {}),
    ([sys.executable, "tools/fixtures.py", "--check"], {}),
    ([sys.executable, "tools/bundled_data.py", "verify"], {}),
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
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--target", "wasm32-wasip1"], {}),
    # Matching opt-in font builds exercise informational diagnostics without
    # changing the default WASI host's data footprint.
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target-dir", "target/host-bundled"], {}),
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--features", "bundled-data", "--target", "wasm32-wasip1", "--target-dir", "target/host-bundled"], {}),
    # Native/WASI parity, the browser shim and the viewer harness (Node, no npm packages).
    (["node", "tools/host_test.mjs"], {}),
    ([sys.executable, "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"], {}),
    ([sys.executable, "tools/conformance.py"], {}),
]

if __name__ == "__main__":
    for command, extra_env in COMMANDS:
        print("Checking: " + " ".join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True, env={**os.environ, **extra_env})
    print("All checks passed. This is not a complete PDF conformance or performance certification.")
