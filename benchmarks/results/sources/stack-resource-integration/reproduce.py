"""Reproduce two native payload defects and the 32-bit spare-cell overflow."""
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
    "native": common + ["fragments::glyphs::tests::", "--", "--nocapture", "--test-threads=1"],
    "wasm-build": common + ["--target", "wasm32-wasip1", "--no-run"],
}
report = {"status": "incomplete", "commands": commands, "exit_codes": {}, "sha256": {}}
try:
    for label, command in list(commands.items()):
        path = local / ("reproducer-001-" + label + ".log")
        with path.open("x") as log:
            code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=600).returncode
        report["exit_codes"][label] = code
        output = path.read_text()
        if label == "native":
            assert code == 101 and "2 failed;" in output, (code, output[-4000:])
            for message in ["retained payload", "warm_row_tables_and_padded_fragments_share_one_budget ... FAILED", "partial_quad_padding_is_admitted_before_counting_rows ... FAILED"]:
                assert message in output, message
        else:
            assert code == 0, output[-4000:]
        artifacts = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
        paths = [a["executable"] for a in artifacts if a.get("reason") == "compiler-artifact" and a.get("target", {}).get("name") == "pagura_render" and a.get("profile", {}).get("test") and a.get("executable")]
        assert len(paths) == 1, paths
        saved = local / ("reproducer-001-" + label + (".wasm" if label == "wasm-build" else ".bin"))
        shutil.copy2(paths[0], saved)
        saved.chmod(0o500)
    command = ["node", str(local / "run_wasi_test.mjs"), str(local / "reproducer-001-wasm-build.wasm"), "fragments::glyphs::tests::row_spare_cells_reject_the_u32_extent_boundary"]
    report["commands"]["wasm-run"] = command
    path = local / "reproducer-001-wasm-run.log"
    with path.open("x") as log:
        code = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=60).returncode
    report["exit_codes"]["wasm-run"] = code
    assert code != 0 and "attempt to add with overflow" in path.read_text(), path.read_text()
    report["status"] = "reproduced-native-budget-and-wasi-overflow"
finally:
    for path in local.glob("reproducer-001-*"):
        if path.is_file():
            report["sha256"][path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
    (local / "reproducer-001.json").write_text(json.dumps(report, indent=2) + "\n")
print(report["status"], flush=True)
