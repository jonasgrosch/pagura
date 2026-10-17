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
    (CARGO + ["test", *OFFLINE, "--workspace", "--doc"], {}),
    (CARGO + ["doc", *OFFLINE, "--workspace", "--no-deps"], {"RUSTDOCFLAGS": "-D warnings"}),
    (CARGO + ["build", *OFFLINE, "--release", "-p", "pagura-host", "--target", "wasm32-wasip1"], {}),
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
