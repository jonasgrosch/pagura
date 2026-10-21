"""Validate imported stroke admission against the retained union reference."""
import os
from pathlib import Path
import subprocess

env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1")
with Path("benchmarks/results/local/stack-resource-integration/stroke-check-001.log").open("x") as log:
    subprocess.run(["cargo", "test", "--locked", "--offline", "--release", "-p",
                    "pagura-raster", "--lib", "stroke::tiling::tests", "-j", "1",
                    "--", "--test-threads=1"], check=True, env=env,
                   stdout=log, stderr=subprocess.STDOUT)
print("Stroke admission checks passed.", flush=True)
