"""Check the corrected glyph payload and extent regressions on native and WASI."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

local = Path("benchmarks/results/local/stack-resource-integration")
env = dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1")
common = ["cargo", "test", "--locked", "--offline", "--release", "-p", "pagura-render", "--lib", "--message-format=json", "-j", "1"]
commands = {
    "native": common + ["fragments::glyphs::tests::", "--", "--test-threads=1"],
    "wasm-build": common + ["--target", "wasm32-wasip1", "--no-run"],
}
report = {"status": "incomplete", "commands": commands, "exit_codes": {}, "sha256": {}}
try:
    for label, command in list(commands.items()):
        path = local / ("fixed-001-" + label + ".log")
        with path.open("x") as log:
            code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=600).returncode
        report["exit_codes"][label] = code
        assert code == 0, path.read_text()[-4000:]
        output = path.read_text()
        if label == "native":
            assert "test result: ok. 12 passed; 0 failed;" in output
        artifacts = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
        paths = [a["executable"] for a in artifacts if a.get("reason") == "compiler-artifact" and a.get("target", {}).get("name") == "pagura_render" and a.get("profile", {}).get("test") and a.get("executable")]
        assert len(paths) == 1, paths
        saved = local / ("fixed-001-" + label + (".wasm" if label == "wasm-build" else ".bin"))
        shutil.copy2(paths[0], saved)
        saved.chmod(0o500)
    command = ["node", str(local / "run_wasi_tests.mjs"), str(local / "fixed-001-wasm-build.wasm"), "fragments::glyphs::tests::"]
    report["commands"]["wasm-run"] = command
    path = local / "fixed-001-wasm-run.log"
    with path.open("x") as log:
        code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=60).returncode
    report["exit_codes"]["wasm-run"] = code
    assert code == 0 and "test result: ok. 12 passed; 0 failed;" in path.read_text(), path.read_text()
    report["status"] = "passed-native-and-wasi-glyph-regressions"
finally:
    for path in local.glob("fixed-001-*"):
        if path.is_file():
            report["sha256"][path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    (local / "fixed-001.json").write_text(json.dumps(report, indent=2) + "\n")
print(report["status"], flush=True)
