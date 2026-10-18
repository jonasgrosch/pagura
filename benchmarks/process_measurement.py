"""Exploratory POSIX process accounting, without a system-time wrapper.

Compatible with benchkit.documents.run_process. wait4 supplies this child's
user/system CPU and peak RSS, never a cumulative maximum from earlier children.
The caller must serialize children; no other thread may reap this Popen child.
"""
import json
import os
import platform
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def run_process(command, policy, *, log_budget=65536):
    """Measure launch through exit; output decoding is outside wall/CPU timing.

    CPU is the final minus initial (zero) per-child user/system accounting.
    Descendants that the child waits for are included by the OS. RSS is the
    largest individual process high-water mark, not an aggregate process tree.
    POSIX wait4 is required; unavailable accounting fails explicitly.
    """
    if not hasattr(os, "wait4"):
        raise RuntimeError("per-child CPU/RSS accounting requires POSIX wait4")
    expired = threading.Event()
    with tempfile.TemporaryFile() as log:
        started = time.perf_counter_ns()
        try:
            child = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=log,
                                     start_new_session=True)
        except OSError as error:
            return {"status": "failed", "exit_code": None,
                    "elapsed_ns": time.perf_counter_ns() - started,
                    "cpu_user_ns": None, "cpu_system_ns": None, "cpu_ns": None,
                    "peak_rss_bytes": None, "diagnostic": str(error)}, ""

        def expire():
            expired.set()
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        remaining = max(0, policy["timeout_seconds"] -
                        (time.perf_counter_ns() - started) / 1e9)
        watchdog = threading.Timer(remaining, expire)
        watchdog.daemon = True
        watchdog.start()
        try:
            _, status, usage = os.wait4(child.pid, 0)
            elapsed = time.perf_counter_ns() - started
            child.returncode = os.waitstatus_to_exitcode(status)
        except BaseException:
            # An interrupted supervisor must not leave a renderer running.
            try:
                os.killpg(child.pid, signal.SIGKILL)
                _, status, _ = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
            except (ProcessLookupError, ChildProcessError):
                pass
            raise
        finally:
            watchdog.cancel()
            watchdog.join()
        log.seek(0)
        diagnostic = log.read(log_budget).decode(errors="replace")
    user, system = round(usage.ru_utime * 1e9), round(usage.ru_stime * 1e9)
    result = {"status": "timeout" if expired.is_set() else "failed",
              "exit_code": child.returncode, "elapsed_ns": elapsed,
              "cpu_user_ns": user, "cpu_system_ns": system,
              "cpu_ns": user + system, "cpu_wall_ratio": (user + system) / elapsed,
              "cpu_parallelism_flag": (user + system) > 1.05 * elapsed,
              "peak_rss_bytes": usage.ru_maxrss * (1 if platform.system() == "Darwin" else 1024),
              "rss_status": "available", "cpu_status": "available"}
    if child.returncode == 3 and not expired.is_set():
        for line in diagnostic.splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict) and record.get("status") == "unsupported" and isinstance(record.get("feature"), str):
                result.update(status="unsupported", feature=record["feature"])
    if child.returncode or expired.is_set():
        result["diagnostic"] = diagnostic[-4000:]
    return result, diagnostic


if __name__ == "__main__":
    # An opt-in front end keeps the portable/default measurement path unchanged.
    import sys
    if "--no-rss" in sys.argv[1:]:
        raise SystemExit("the wait4 front end always records RSS; omit --no-rss")
    sys.path.insert(0, str(ROOT / "tools"))
    import bench
    import benchkit.documents
    import benchkit.measurement
    benchkit.documents.run_process = run_process
    benchkit.measurement.run_process = run_process
    bench.main()
