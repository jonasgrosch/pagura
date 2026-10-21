#!/usr/bin/env python3
"""Exercise the exact campaign script with tiny owned Python processes only.

No renderer/build runs. Mock only input pins, tools/bench.py, and the campaign
Timer interval (1200 -> 3 seconds). Use the real ProcessGroup and run_process.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

SHIM = r'''import runpy, sys, threading
from pathlib import Path
sys.dont_write_bytecode = True
source, tools = sys.argv[1:]
sys.path.insert(0, tools)
original = threading.Timer
def accelerated(interval, callback, *args, **kwargs):
    if interval != 1200:
        raise AssertionError(f"unexpected campaign timer: {interval}")
    return original(3, callback, *args, **kwargs)
threading.Timer = accelerated
runpy.run_path(source, run_name="__main__")
'''
CONVERSION = r'''import json, os, signal, sys
from pathlib import Path
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ["PROBE_TOOLS"])
from benchkit.measurement import run_process, MeasurementCancelled
root = Path(os.environ["PROBE_DIRECTORY"])
(root / "conversion.pid").write_text(str(os.getpid()))
previous = {value: signal.getsignal(value) for value in (signal.SIGINT, signal.SIGTERM)}
try:
    result, log = run_process([sys.executable, "-B", str(root / "renderer.py")],
                              {"timeout_seconds": 8, "measure_rss": False})
except MeasurementCancelled as error:
    restored = all(signal.getsignal(value) == handler for value, handler in previous.items())
    (root / "conversion-cancelled.json").write_text(json.dumps({"signal": error.signum, "handlers_restored": restored}))
    raise
else:
    (root / "later-sample.txt").write_text(json.dumps(result))
finally:
    (root / "conversion-stopped.txt").write_text("owned conversion exiting")
'''
RENDERER = r'''import os, signal, time
from pathlib import Path
root = Path(os.environ["PROBE_DIRECTORY"])
def stop(signum, frame):
    time.sleep(0.15)
    (root / "renderer-stopped.txt").write_text(str(signum))
    raise SystemExit(0)
signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
(root / "renderer.pid").write_text(str(os.getpid()))
(root / "renderer-ready.txt").write_text("ready")
# Emergency self-exit bounds the fake job even if ownership cleanup regresses.
end = time.monotonic() + 10
while time.monotonic() < end:
    time.sleep(0.05)
raise SystemExit(5)
'''


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def scenario(base, campaign, tools, mode):
    root = base / mode
    local = root / "benchmarks/results/local/analytic19-conversion"
    sources = root / "benchmarks/results/sources/analytic19-conversion"
    local.mkdir(parents=True)
    sources.mkdir(parents=True)
    (root / "tools").mkdir()
    (sources / "inputs.json").write_text(json.dumps({"files": {}, "product_baseline_commit": "mock-probe-only"}))
    (root / "tools/bench.py").write_text(CONVERSION)
    (root / "renderer.py").write_text(RENDERER)
    (root / "shim.py").write_text(SHIM)
    env = {**os.environ, "PROBE_DIRECTORY": str(root), "PROBE_TOOLS": str(tools),
           "PYTHONDONTWRITEBYTECODE": "1"}
    result = {"mode": mode, "status": "incomplete"}
    child = None
    started = time.monotonic()
    try:
        with (root / "outer.log").open("xb") as log:
            child = subprocess.Popen([sys.executable, "-B", str(root / "shim.py"), str(campaign), str(tools)],
                                     cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            # Both scenarios must reach the detached renderer before cancellation.
            until = time.monotonic() + 2.5
            while not (root / "renderer-ready.txt").exists() and child.poll() is None and time.monotonic() < until:
                time.sleep(0.01)
            require((root / "renderer-ready.txt").exists(), "nested renderer did not reach readiness before deadline")
            if mode == "external-sigterm":
                child.send_signal(signal.SIGTERM)
            result["outer_exit_code"] = child.wait(timeout=15)
        record = json.loads((local / "campaign-001.json").read_text())
        cancelled = json.loads((root / "conversion-cancelled.json").read_text())
        require(record["status"] == "command-incomplete", "cancellation reported successful campaign")
        require(record.get("cleanup_verified") is True, "campaign cleanup was not verified")
        require(record.get("cancelled_signal") == signal.SIGTERM, "wrong cancellation signal")
        require(record.get("deadline_expired") is (mode == "deadline"), "deadline classification differs")
        require(record.get("exit_code") == 128 + signal.SIGTERM, "conversion cancellation did not propagate")
        require(cancelled == {"signal": signal.SIGTERM, "handlers_restored": True}, "nested handler restoration failed")
        require((root / "renderer-stopped.txt").read_text() == str(signal.SIGTERM), "renderer did not shut down gracefully")
        require((root / "conversion-stopped.txt").exists(), "conversion cleanup marker missing")
        require(not (root / "later-sample.txt").exists(), "conversion started another sample after cancellation")
        pids = {name: int((root / (name + ".pid")).read_text()) for name in ("conversion", "renderer")}
        require(not any(alive(pid) for pid in pids.values()), "an owned process survived campaign exit")
        require(result["outer_exit_code"] != 0, "canceled campaign exited successfully")
        result.update(status="passed", campaign=record, pids=pids)
    except (OSError, ValueError, KeyError, AssertionError, subprocess.TimeoutExpired) as error:
        result.update(status="failed", error=f"{type(error).__name__}: {error}")
    finally:
        if child is not None and child.poll() is None:
            # Signal only our direct child. Nested fake jobs also have independent
            # 8/10-second expiry, so a broken cascade does not leave a long job.
            child.send_signal(signal.SIGTERM)
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                result["cleanup_error"] = "owned campaign still active; inspect retained PIDs"
                result["status"] = "failed"
        result["elapsed_seconds"] = time.monotonic() - started
        result["artifacts"] = {p.relative_to(root).as_posix(): sha(p.read_bytes())
                               for p in root.rglob("*") if p.is_file()}
        (root / "probe-result.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--campaign", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    root, source = args.source_root.resolve(strict=True), args.campaign.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    code = source.read_bytes()
    retained = output / "campaign.py"
    retained.write_bytes(code)
    helper_paths = [root / "tools/benchkit/process_group.py", root / "tools/benchkit/measurement.py"]
    helpers_before = {str(path): sha(path.read_bytes()) for path in helper_paths}
    results = [scenario(output, retained, root / "tools", mode) for mode in ("external-sigterm", "deadline")]
    unchanged = source.read_bytes() == code and all(sha(Path(path).read_bytes()) == digest for path, digest in helpers_before.items())
    report = {"schema": 1, "status": "passed" if unchanged and all(row["status"] == "passed" for row in results) else "failed",
              "campaign_sha256": sha(code), "campaign_source": str(source), "helpers_sha256": helpers_before,
              "sources_unchanged": unchanged, "cases": results,
              "scope": "exact campaign lifecycle; mocked pins/conversion; real nested run_process and ProcessGroup; timer1200 shortened to3s; no product rendering/build"}
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "report": str(output / "report.json")}, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
