"""Verify retained reconciliation evidence and write its scoped validation record."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path.cwd()
LOCAL = ROOT / "benchmarks/results/local/main-reconciliation"
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.config import source_digest


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(Path(path).read_text())


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


pins = {}


def pin(path):
    path = Path(path).resolve()
    pins[str(path)] = sha(path)
    return pins[str(path)]


source = source_digest()
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
main = subprocess.check_output(["git", "rev-parse", "MERGE_HEAD"], text=True).strip()
require(head == "237f134ac59bb1f064abfd7cb98375f71f4ef919", "unexpected first parent")
require(main == "d81d0c82612b70934d076e327ca5a154306bf487", "unexpected merged main")
checks = {}
for label in ("focused-001", "full-check-001", "wasi-simd-001", "wasi-scalar-001", "parity-001"):
    receipt_path = LOCAL / (label + "-receipt.json")
    guard_path = LOCAL / (label + "-observer/guard.json")
    receipt, guard = read(receipt_path), read(guard_path)
    require(receipt["status"] == guard["status"] == "completed", label + " incomplete")
    require(receipt["exit_code"] == receipt["wrapper_exit_code"] == guard["exit_code"]
            == guard["child_exit_code"] == 0, label + " command failed")
    require(receipt["kind"] == "validation" and not receipt["errors"]
            and not receipt["artifact_errors"] and receipt["cancellation_signal"] is None,
            label + " runner failure")
    require(receipt["source_unchanged"] and receipt["source_before"] == receipt["source_after"]
            == {"commit": head, "sha256": source}, label + " source mismatch")
    require(guard["observation_only"] is False and guard["poll_seconds"] == 5
            and [event["stage"] for event in guard["events"]] == ["preflight", "postflight"]
            and all(not event["matches"] for event in guard["events"]), label + " contention")
    require(receipt["argv"] == ["python3", "-B", "benchmarks/results/sources/analytic19-conversion/load_observer.py",
            "--evidence-dir", str(guard_path.parent.relative_to(ROOT)), "--", *guard["command"]],
            label + " observer launch mismatch")
    for path, digest in receipt["artifacts"].items():
        require(pin(path) == digest, label + " artifact drift")
    for event in guard["events"]:
        pin(guard_path.parent / event["snapshot"])
    pin(receipt_path)
    pin(guard_path)
    if label != "parity-001":
        report_path, log_path = LOCAL / (label + ".json"), LOCAL / (label + ".log")
        require({str(report_path), str(log_path), str(guard_path)} <= set(receipt["artifacts"]),
                label + " missing receipt binding")
        report = read(report_path)
        check_path = "benchmarks/results/sources/main-reconciliation/check.py"
        require(report["driver_sha256"] == sha(ROOT / check_path), label + " command wrapper changed")
        require(guard["command"] == ["python3", "-B", check_path, label, *report["command"]],
                label + " command launch mismatch")
        require(report["status"] == "command-passed" and report["source_unchanged"]
                and report["source_before"] == report["source_after"] == source
                and report["run_id"] == receipt["run_id"] and report["process"]["exit_code"] == 0,
                label + " report mismatch")
        require(report["log_sha256"] == pin(log_path), label + " log mismatch")
        pin(report_path)
        log = log_path.read_text()
        if label == "full-check-001":
            require(report["command"] == ["python3", "-B", "tools/check.py", "--jobs", "1"], "wrong full check")
            for marker in ("running 142 tests", "running 229 tests", "Ran 196 tests",
                           "Conformance: 1249 passed, 0 failed of 1249 cases", "All checks passed."):
                require(marker in log, "missing full-check evidence: " + marker)
        else:
            require("test result: ok. 5 passed; 0 failed;" in log, label + " missing sampler tests")
        if label == "focused-001":
            require(report["command"] == ["cargo", "test", "--locked", "--offline", "--release", "-j", "1",
                    "-p", "pagura-render", "--lib", "compositor::opaque_image::tests", "--", "--test-threads=1"],
                    "wrong native sampler check")
        if label.startswith("wasi-"):
            platform = "simd" if label == "wasi-simd-001" else "scalar"
            flags = "+simd128" if platform == "simd" else "-simd128"
            require(report["command"] == ["env", "-u", "CARGO_ENCODED_RUSTFLAGS",
                    "RUSTFLAGS=-C target-feature=" + flags,
                    "CARGO_TARGET_WASM32_WASIP1_RUNNER=node " + str(ROOT / "benchmarks/results/sources/main-reconciliation/run_wasi_tests.mjs"),
                    "cargo", "test", "--locked", "--offline", "--release", "-j", "1", "-p", "pagura-render", "--lib",
                    "--target", "wasm32-wasip1", "--target-dir", "target/main-reconciliation-wasi-" + platform,
                    "compositor::opaque_image::tests", "--", "--test-threads=1"], "wrong WASI build or flags")
            modules = [json.loads(line) for line in log.splitlines() if line.startswith('{"schema":1,"module":')]
            require(len(modules) == 1, label + " missing executed module pin")
            module = modules[0]
            require(pin(module["module"]["path"]) == module["module"]["sha256"], "module changed")
            require(module["test_args"] == ["compositor::opaque_image::tests", "--test-threads=1"], "wrong test filter")
        checks[label] = {"report": report, "receipt_sha256": sha(receipt_path), "guard_sha256": sha(guard_path),
                         "observer_scans": guard["scans"]}
    else:
        require(guard["command"] == ["/opt/homebrew/bin/python3", "-B",
                str(ROOT / "benchmarks/results/sources/main-reconciliation/parity.py"), "--root", str(ROOT),
                "--output", str(LOCAL / "parity-001"), "--cargo", "/Users/jonasgrosch/.cargo/bin/cargo",
                "--node", "/opt/homebrew/bin/node"], "wrong parity launch")
        require({str(LOCAL / "parity-001/report.json"), str(LOCAL / "parity-001/plan.json"),
                 str(guard_path)} <= set(receipt["artifacts"]), "missing parity receipt binding")
        checks[label] = {"receipt_sha256": sha(receipt_path), "guard_sha256": sha(guard_path),
                         "observer_scans": guard["scans"]}

parity_root = LOCAL / "parity-001"
parity_path = parity_root / "report.json"
parity = read(parity_path)
parity_receipt = read(LOCAL / "parity-001-receipt.json")
require(parity["run_id"] == parity_receipt["run_id"]
        and parity["receipt"] == str(LOCAL / "parity-001-receipt.json"), "parity runner mismatch")
require(parity["status"] == "passed-all-24" and len(parity["samples"]) == 24
        and not parity["failures"] and len(parity["parity"]) == 8
        and all(item["equal"] for item in parity["parity"]), "parity matrix failed")
require(parity["source_before"] == parity["source_after"], "parity source changed")
for relative, record in parity["artifacts"].items():
    path = (parity_root / relative).resolve()
    require(path.is_relative_to(parity_root.resolve()) and path.stat().st_size == record["bytes"]
            and pin(path) == record["sha256"], "parity artifact mismatch")
for path, digest in parity["bound_files_sha256"].items():
    require(pin(path) == digest, "parity helper/runtime drift")
cases = {"paint/image-direct-" + name for name in ("binary-halves", "gray-quarter-turn", "rgb-fractional", "bilinear")}
expected_keys = {(case, mode, platform) for case in cases for mode in ("exact", "competitive")
                 for platform in ("native", "wasi-simd", "wasi-scalar")}
seen, outputs = set(), []
for sample in parity["samples"]:
    key = sample["case"], sample["mode"], sample["platform"]
    require(key in expected_keys and key not in seen and sample["status"] == "passed", "wrong sample matrix")
    seen.add(key)
    ppm = (parity_root / "oracles" / (sample["case"] + ".ppm")).read_bytes()
    require(ppm.startswith(b"P6\n32 32\n255\n") and len(ppm) == 13 + 32 * 32 * 3, "wrong oracle")
    rgb = ppm[13:]
    expected = b"RGBA8 32 32 premultiplied\n" + b"".join(rgb[i:i + 3] + b"\xff" for i in range(0, len(rgb), 3))
    path = parity_root / "samples" / sample["case"] / sample["mode"] / sample["platform"] / "output.rgba"
    require(path.read_bytes() == expected and sha(path) == sample["output_sha256"], "independent pixel mismatch")
    outputs.append({"case": key[0], "mode": key[1], "platform": key[2], "sha256": sha(path)})
require(seen == expected_keys, "missing output")
for case in cases:
    for mode in ("exact", "competitive"):
        require(len({row["sha256"] for row in outputs if row["case"] == case and row["mode"] == mode}) == 1,
                "cross-platform mismatch")
pin(parity_path)
for path in (ROOT / "benchmarks/results/sources/main-reconciliation").iterdir():
    if path.is_file():
        pin(path)
require(source_digest() == source, "source changed during evidence review")
summary = {"schema": 1, "status": "validated-main-reconciliation", "admission": False,
           "first_parent": head, "merged_main": main, "runner_source_sha256": source,
           "parity_product_identity": parity["source_before"], "checks": checks,
           "parity_outputs": outputs, "evidence_sha256": pins,
           "limits": ["No speed, memory, release or global accuracy claim.",
                      "Four image scenes only for direct cross-platform output comparison; internal sampler tests cover irregular tiles.",
                      "The historic four main tie-byte cases were not reconstructed.",
                      "Five-second known-process observation may miss short or unrecognized jobs.",
                      "Historical benchmark identities and failed gates are unchanged."]}
destination = ROOT / "benchmarks/results/main-reconciliation.json"
with destination.open("x") as handle:
    json.dump(summary, handle, indent=2)
    handle.write("\n")
print(destination)
