"""Run the frozen analytic conversion pilot once with owned-child cleanup."""
import hashlib
import json
import os
import signal
import subprocess
import threading
import time
from pathlib import Path
import sys

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.process_group import ProcessGroup, require_group_support

LOCAL = ROOT / "benchmarks/results/local/analytic19-conversion"
SOURCES = ROOT / "benchmarks/results/sources/analytic19-conversion"
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
pins = json.loads((SOURCES / "inputs.json").read_text())
assert all(sha(ROOT / path) == digest for path, digest in pins["files"].items())
assert not (LOCAL / "conversion-001.json").exists(), "refuse to overwrite a trial"
os.environ.update(CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1", PYTHONDONTWRITEBYTECODE="1")
command = [sys.executable, "-B", "tools/bench.py", "convert", "--corpus", str(ROOT),
           "--manifest", str(SOURCES / "manifest.json"), "--process-per-page",
           "--pages", "all", "--dpis", "72", "--engines",
           "pagura,pdfium,mupdf,pdfjs,hayro,quartz,poppler",
           "--samples", "5", "--warmups", "2", "--seed", "20261021",
           "--output", str(LOCAL / "conversion-001.json")]
record = {"schema": 1, "status": "incomplete", "command": command,
          "deadline_seconds": 1200, "inputs_sha256": sha(SOURCES / "inputs.json"),
          "product_baseline_commit": pins["product_baseline_commit"],
          "scope": "one fixed diagnostic pilot; no retry or release admission"}
require_group_support()
child, group, cancelled, expired = None, None, None, False
started = time.monotonic()

def cancel(signum, _frame=None):
    global cancelled
    if cancelled is None:
        cancelled = signum
    if group is not None:
        group.forward(signum)

def expire():
    global expired
    expired = True
    # Graceful forwarding is essential: the conversion owner must cancel
    # its active renderer, which lives in a separate process group.
    cancel(signal.SIGTERM)

previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
watchdog = threading.Timer(1200, expire)
watchdog.daemon = True
watchdog_started = False
try:
    for signum in previous:
        signal.signal(signum, cancel)
    watchdog.start()
    watchdog_started = True
    if cancelled is None:
        with (LOCAL / "campaign-001.log").open("x") as log:
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            group = ProcessGroup(child)
            record["child_pid"] = child.pid
            record["cleanup_verified"] = False
            if cancelled is not None:
                group.forward(cancelled)
            group.wait_leader()
            record["drained_descendants"] = group.drain()
            record["cleanup_verified"] = True
finally:
    try:
        if group is not None and not group.leader_exited:
            group.forward(cancelled or signal.SIGTERM)
            group.wait_leader()
            record["drained_descendants"] = group.drain()
            record["cleanup_verified"] = True
    finally:
        if watchdog_started:
            watchdog.cancel()
            watchdog.join()
        try:
            if group is not None:
                record["exit_code"] = group.reap()
                verified = record.get("cleanup_verified", False)
                record["cleanup_verified"] = False
                group.check()
                record["cleanup_verified"] = verified
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)
            record["elapsed_seconds"] = time.monotonic() - started
            record["cancelled_signal"] = cancelled
            record["deadline_expired"] = expired
            record["status"] = ("command-completed" if record.get("exit_code") == 0
                                and cancelled is None and record.get("cleanup_verified") is True
                                else "command-incomplete")
            record["inputs_unchanged"] = all(sha(ROOT / path) == digest
                                            for path, digest in pins["files"].items())
            record["artifacts_sha256"] = {name: sha(LOCAL / name) for name in
                ("conversion-001.json", "conversion-001.md", "campaign-001.log")
                if (LOCAL / name).is_file()}
            (LOCAL / "campaign-001.json").write_text(json.dumps(record, indent=2) + "\n")
assert record["status"] == "command-completed", record["status"]
assert record["inputs_unchanged"], "frozen sources changed during pilot"
print("Fixed analytic19 campaign command completed; independent audit still required.", flush=True)
