"""Exploratory POSIX process accounting, with isolated engine RSS on Linux.

Compatible with benchkit.documents.run_process. wait4 supplies this child's
user/system CPU and peak RSS, never a cumulative maximum from earlier children.
The caller must serialize children; no other thread may reap this child.
"""
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LINUX_LAUNCHER = Path(__file__).with_name("linux_process.py")


def measurement_options():
    """Record the Linux launcher's measured RSS floor once per matrix run."""
    options = {"cpu_enabled": True, "rss_enabled": True, "accounting": "POSIX wait4 per child"}
    if sys.platform == "linux":
        floor, _ = run_process(["/bin/true"], {"timeout_seconds": 10})
        if floor["exit_code"] != 0 or floor.get("rss_status") != "available":
            raise RuntimeError("cannot measure isolated Linux launcher RSS floor")
        options.update(accounting="POSIX wait4; engine timing and RSS from isolated supervisor",
                       rss_floor_bytes=floor["peak_rss_bytes"], rss_floor_command=["/bin/true"],
                       rss_launcher=[sys.executable, "-I", "-S", str(LINUX_LAUNCHER)])
    return options


def run_process(command, policy, *, log_budget=65536):
    """Measure launch through exit; output decoding is outside wall/CPU timing.

    CPU is the final minus initial (zero) per-child user/system accounting.
    On Linux, wall/CPU/RSS come from the isolated supervisor's engine interval;
    supervisor startup, imports, accounting serialization and exit are excluded.
    Descendants that the child waits for are included by the OS. RSS is the
    largest individual process high-water mark, not an aggregate process tree.
    POSIX wait4 is required; unavailable accounting fails explicitly.
    """
    if not hasattr(os, "wait4"):
        raise RuntimeError("per-child CPU/RSS accounting requires POSIX wait4")
    expired = threading.Event()
    child, returncode = None, None
    linux = sys.platform == "linux"
    with tempfile.TemporaryFile() as log, tempfile.TemporaryFile() as accounting:
        started = time.perf_counter_ns()
        try:
            launched = ([sys.executable, "-I", "-S", str(LINUX_LAUNCHER), str(accounting.fileno()), *command]
                        if linux else command)
            child = subprocess.Popen(launched, cwd=ROOT, stdout=log, stderr=log,
                                     start_new_session=True, pass_fds=(accounting.fileno(),) if linux else ())
            pid = child.pid
        except OSError as error:
            return {"status": "failed", "exit_code": None,
                    "elapsed_ns": time.perf_counter_ns() - started,
                    "cpu_user_ns": None, "cpu_system_ns": None, "cpu_ns": None,
                    "peak_rss_bytes": None, "diagnostic": str(error)}, ""

        def expire():
            expired.set()
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

        remaining = max(0, policy["timeout_seconds"] -
                        (time.perf_counter_ns() - started) / 1e9)
        watchdog = threading.Timer(remaining, expire)
        watchdog.daemon = True
        watchdog.start()
        try:
            _, status, usage = os.wait4(pid, 0)
            elapsed = time.perf_counter_ns() - started
            returncode = os.waitstatus_to_exitcode(status)
        except BaseException:
            # An interrupted supervisor must not leave a renderer running.
            try:
                os.killpg(pid, signal.SIGKILL)
                _, status, _ = os.wait4(pid, 0)
                returncode = os.waitstatus_to_exitcode(status)
            except (ProcessLookupError, ChildProcessError):
                pass
            raise
        finally:
            watchdog.cancel()
            watchdog.join()
            if child is not None:
                child.returncode = returncode
        log.seek(0)
        diagnostic = log.read(log_budget).decode(errors="replace")
        user, system = round(usage.ru_utime * 1e9), round(usage.ru_stime * 1e9)
        peak = usage.ru_maxrss * (1 if platform.system() == "Darwin" else 1024)
        accounting_error = None
        if linux:
            # A killed supervisor cannot report the engine's RSS. Never substitute
            # the supervisor's wait4 peak: it contains the harness's address space.
            peak = None
            if not expired.is_set():
                try:
                    accounting.seek(0)
                    record = accounting.read(4096).decode()
                    if returncode != 0:
                        raise ValueError("isolated Linux supervisor failed")
                    if record.startswith("error "):
                        raise ValueError(record[6:].strip())
                    fields = record.split()
                    if len(fields) != 6 or fields[0] != "ok":
                        raise ValueError("invalid engine accounting record")
                    code, wall, cpu_user, cpu_system, rss = map(int, fields[1:])
                    if wall <= 0 or cpu_user < 0 or cpu_system < 0 or rss <= 0:
                        raise ValueError("invalid engine timing or RSS")
                    returncode, elapsed, user, system, peak = code, wall, cpu_user, cpu_system, rss
                except ValueError as error:
                    accounting_error = f"Linux child accounting failed: {error}"
    result = {"status": "timeout" if expired.is_set() else "failed",
              "exit_code": returncode, "elapsed_ns": elapsed,
              "cpu_user_ns": user, "cpu_system_ns": system,
              "cpu_ns": user + system, "cpu_wall_ratio": (user + system) / elapsed,
              "cpu_parallelism_flag": (user + system) > 1.05 * elapsed,
              "peak_rss_bytes": peak,
              "rss_status": "available" if peak is not None else "unavailable", "cpu_status": "available"}
    if accounting_error or (linux and expired.is_set()):
        result.update(cpu_user_ns=None, cpu_system_ns=None, cpu_ns=None,
                      cpu_wall_ratio=None, cpu_parallelism_flag=False, cpu_status="unavailable",
                      peak_rss_bytes=None, rss_status="unavailable")
    if accounting_error:
        result.update(exit_code=None, diagnostic=accounting_error)
        return result, diagnostic
    if returncode == 3 and not expired.is_set():
        for line in diagnostic.splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict) and record.get("status") == "unsupported" and isinstance(record.get("feature"), str):
                result.update(status="unsupported", feature=record["feature"])
    if returncode or expired.is_set():
        result["diagnostic"] = diagnostic[-4000:]
    return result, diagnostic


if __name__ == "__main__":
    # An opt-in front end keeps the portable/default measurement path unchanged.
    if "--no-rss" in sys.argv[1:]:
        raise SystemExit("the wait4 front end always records RSS; omit --no-rss")
    sys.path.insert(0, str(ROOT / "tools"))
    import bench
    import benchkit.documents
    import benchkit.measurement
    benchkit.documents.run_process = run_process
    benchkit.measurement.run_process = run_process
    bench.main()
