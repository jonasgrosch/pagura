"""Validate comment-only clarifications after the frozen integration check."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

local = Path("benchmarks/results/local/stack-resource-integration")
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
inputs = json.loads((local / "post-doc-inputs.json").read_text())
assert all(sha(path) == value for path, value in inputs.items())
env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1", RUSTDOCFLAGS="-D warnings")
commands = {
    "fmt": ["cargo", "fmt", "--all", "--", "--check"],
    "doc": ["cargo", "doc", "--locked", "--offline", "-p", "pagura-render", "--no-deps", "-j", "1"],
}
report = {"status": "incomplete", "commands": commands, "exit_codes": {},
          "script_sha256": sha(__file__), "inputs_sha256": sha(local / "post-doc-inputs.json")}
try:
    for label, command in commands.items():
        with (local / ("post-doc-001-" + label + ".log")).open("x") as output:
            code = subprocess.run(command, env=env, stdout=output, stderr=subprocess.STDOUT).returncode
        report["exit_codes"][label] = code
        assert code == 0, (label, code)
    report["status"] = "passed-comment-only-doc-validation"
finally:
    report["inputs_unchanged"] = all(sha(path) == value for path, value in inputs.items())
    report["logs_sha256"] = {p.name: sha(p) for p in local.glob("post-doc-001-*.log")}
    (local / "post-doc-001.json").write_text(json.dumps(report, indent=2) + "\n")
assert report["inputs_unchanged"]
print(report["status"], flush=True)
