"""Validate the frozen integration and replay its glyph regressions on WASI."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

local = Path("benchmarks/results/local/stack-resource-integration")
inputs = json.loads((local / "final-inputs.json").read_text())
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
assert all(sha(path) == digest for path, digest in inputs.items())
env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1")
commands = {
    "full": ["python3", "tools/check.py", "--jobs", "1"],
    "wasm-build": ["cargo", "test", "--locked", "--offline", "--release", "-p", "pagura-render", "--lib", "--message-format=json", "-j", "1", "--target", "wasm32-wasip1", "--no-run"],
}
report = {"status": "incomplete", "commands": commands, "exit_codes": {}, "inputs_sha256": sha(local / "final-inputs.json"), "sha256": {}}
try:
    for label, command in list(commands.items()):
        path = local / ("full-check-002-" + label + ".log")
        with path.open("x") as log:
            code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT).returncode
        report["exit_codes"][label] = code
        assert code == 0, (label, code, str(path))
    output = path.read_text()
    artifacts = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
    paths = [a["executable"] for a in artifacts if a.get("reason") == "compiler-artifact" and a.get("target", {}).get("name") == "pagura_render" and a.get("profile", {}).get("test") and a.get("executable")]
    assert len(paths) == 1, paths
    saved = local / "full-check-002-glyphs.wasm"
    shutil.copy2(paths[0], saved)
    saved.chmod(0o400)
    command = ["node", str(local / "run_wasi_tests.mjs"), str(saved), "fragments::glyphs::tests::"]
    report["commands"]["wasm-run"] = command
    path = local / "full-check-002-wasm-run.log"
    with path.open("x") as log:
        code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=60).returncode
    report["exit_codes"]["wasm-run"] = code
    assert code == 0 and "test result: ok. 12 passed; 0 failed;" in path.read_text()
    report["status"] = "passed-full-check-and-wasi-regressions"
finally:
    report["inputs_unchanged"] = all(sha(path) == digest for path, digest in inputs.items())
    for path in local.glob("full-check-002-*"):
        if path.is_file():
            report["sha256"][path.name] = sha(path)
    (local / "full-check-002.json").write_text(json.dumps(report, indent=2) + "\n")
assert report["inputs_unchanged"]
print(report["status"], flush=True)
