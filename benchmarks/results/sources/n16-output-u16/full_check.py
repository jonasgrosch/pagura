import os, subprocess
from pathlib import Path
env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1")
with Path("benchmarks/results/local/n16-output-u16/full-check-001.log").open("x") as log:
    subprocess.run(["python3", "tools/check.py", "--jobs", "1"], check=True, env=env, stdout=log, stderr=subprocess.STDOUT)
print("Full repository check passed.", flush=True)
