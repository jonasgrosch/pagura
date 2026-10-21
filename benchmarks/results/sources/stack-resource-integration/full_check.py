"""Run the integrated product check once with bounded local concurrency."""
import os
from pathlib import Path
import subprocess

env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1")
with Path("benchmarks/results/local/stack-resource-integration/full-check-001.log").open("x") as log:
    subprocess.run(["python3", "tools/check.py", "--jobs", "1"], check=True,
                   env=env, stdout=log, stderr=subprocess.STDOUT)
print("Full integrated repository check passed.", flush=True)
