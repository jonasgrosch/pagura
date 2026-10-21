"""Check renderer lint requirements before the complete repository check."""
import os
from pathlib import Path
import subprocess
with Path("benchmarks/results/local/stack-resource-integration/clippy-001.log").open("x") as log:
    subprocess.run(["cargo", "clippy", "--locked", "--offline", "-p", "pagura-render", "--all-targets", "-j", "1", "--", "-D", "warnings"], check=True,
                   env=dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1"), stdout=log, stderr=subprocess.STDOUT)
print("Renderer Clippy passed.", flush=True)
