"""Process timing, bounded output verification and failure evidence."""
import os
import json
import platform
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from .config import ROOT, sha
from .images import normalize, verify_pixels
from .timings import parse_stages
from .artifacts import read_bounded_regular


class MeasurementCancelled(SystemExit):
    """Cancellation escapes the sample loop only after owned children stop."""

    def __init__(self, signum):
        self.signum = signum
        super().__init__(128 + signum)


def rss_wrapper():
    """Per-child RSS instrumentation; absence is recorded and blocks release acceptance."""
    if platform.system() == "Darwin":
        return ["/usr/bin/time", "-l"]
    if platform.system() == "Linux" and Path("/usr/bin/time").exists():
        return ["/usr/bin/time", "-f", "PAGURA_RSS_KIB=%M"]
    return []


def run_process(command, policy, *, log_budget=65536):
    """Run once with timeout and per-child RSS; verification follows process exit."""
    if threading.current_thread() is not threading.main_thread():
        raise ValueError("measurement requires main-thread cancellation handling")
    with tempfile.TemporaryFile() as log:
        system = platform.system()
        wrapper = rss_wrapper() if policy.get("measure_rss", True) else []
        timed_out, process, cancelled = False, None, None

        def forward(signum):
            if process is not None:
                try:
                    if os.name != "nt":
                        os.killpg(process.pid, signum)
                    elif process.poll() is None:
                        process.send_signal(signum)
                except ProcessLookupError:
                    pass

        def cancel(signum, _frame):
            nonlocal cancelled
            if cancelled is None:
                cancelled = signum
            forward(signum)

        def expire():
            nonlocal timed_out
            timed_out = True
            if process is not None:
                try:
                    if os.name != "nt":
                        os.killpg(process.pid, signal.SIGKILL)
                    elif process.poll() is None:
                        process.kill()
                except ProcessLookupError:
                    pass

        def group_alive():
            if os.name == "nt":
                return process.poll() is None
            try:
                os.killpg(process.pid, 0)
                return True
            except ProcessLookupError:
                return False

        # A blocking wait avoids the millisecond polling steps of wait(timeout=...).
        # Watchdog setup is common overhead; its join is outside the timed region.
        previous = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
        watchdog = None
        watchdog_started = False
        try:
            # Install before spawn: a signal during Popen is retained and forwarded
            # as soon as its owned process-group ID is known.
            for signum in previous:
                signal.signal(signum, cancel)
            if cancelled is not None:
                raise MeasurementCancelled(cancelled)
            started = time.perf_counter_ns()
            try:
                process = subprocess.Popen(wrapper + command, cwd=ROOT, stdout=log, stderr=log,
                                           start_new_session=os.name != "nt")
            except OSError as error:
                if cancelled is not None:
                    raise MeasurementCancelled(cancelled) from error
                return ({"status": "failed", "exit_code": None, "elapsed_ns": time.perf_counter_ns() - started,
                         "peak_rss_bytes": None, "diagnostic": str(error)}, "")
            remaining = max(0, policy["timeout_seconds"] - (time.perf_counter_ns() - started) / 1e9)
            watchdog = threading.Timer(remaining, expire)
            watchdog.daemon = True
            watchdog.start()
            watchdog_started = True
            if cancelled is not None:
                forward(cancelled)
            try:
                process.wait()
            except KeyboardInterrupt:
                cancel(signal.SIGINT, None)
                process.wait()
            elapsed = time.perf_counter_ns() - started
            # Keep the watchdog active until the detached group has gone,
            # including descendants surviving a normally exited leader.
            drained = group_alive()
            while group_alive():
                time.sleep(0.01)
            if drained:
                # A descendant may still be producing output. Include its work
                # and conservative polling overhead in the end-to-end elapsed.
                elapsed = time.perf_counter_ns() - started
            if cancelled is not None:
                raise MeasurementCancelled(cancelled)
        except BaseException:
            # A Python interruption/error after spawn must not strand the owned
            # renderer group. Real SIGINT/SIGTERM use the scoped handler above.
            if process is not None:
                if not watchdog_started:
                    # Failure to start a watchdog cannot leave an unbounded
                    # graceful wait or a renderer orphan during error handling.
                    expire()
                else:
                    forward(cancelled if cancelled is not None else signal.SIGTERM)
                process.wait()
                while group_alive():
                    time.sleep(0.01)
            raise
        finally:
            try:
                if watchdog_started:
                    watchdog.cancel()
                    watchdog.join()
            finally:
                for signum, handler in previous.items():
                    signal.signal(signum, handler)
        if cancelled is not None:
            raise MeasurementCancelled(cancelled)
        log.seek(0)
        diagnostic = log.read(log_budget).decode(errors="replace")
    rss_values = re.findall(r"(\d+)\s+maximum resident set size", diagnostic) if system == "Darwin" else re.findall(r"PAGURA_RSS_KIB=(\d+)", diagnostic)
    result = {"status": "timeout" if timed_out else "failed", "exit_code": process.returncode,
              "elapsed_ns": elapsed, "peak_rss_bytes": int(rss_values[-1]) * (1024 if system == "Linux" else 1) if rss_values else None}
    if drained:
        result["timing_note"] = "includes owned process-group drain and polling overhead"
    result["rss_status"] = "available" if result["peak_rss_bytes"] is not None else "unavailable"
    if process.returncode == 3 and not timed_out:
        for line in diagnostic.splitlines():
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if isinstance(record, dict) and record.get("status") == "unsupported" and isinstance(record.get("feature"), str):
                result.update(status="unsupported", feature=record["feature"])
    if process.returncode or timed_out:
        result["diagnostic"] = diagnostic[-4000:]
        return result, diagnostic
    return result, diagnostic


def measure(command, output, expected, policy, *, page=None, stages_required=False, image_sink=None, tolerance=0,
            artifact_store=None, execution_context=None):
    """Time the whole adapter process; decode/verify its raw output after timing.

    Peak RSS comes from this child, never cumulative resource.getrusage values.
    Timeout kills its process group on POSIX. Untrusted output is bounded on read.
    """
    output.unlink(missing_ok=True)
    if artifact_store is not None:
        try:
            artifact_store.before_launch(execution_context, command)
        except (OSError, ValueError, TypeError) as error:
            artifact_store.incomplete(error)
            return {"status": "invalid-evidence", "exit_code": None, "artifact_error": str(error)}
    result, diagnostic = run_process(command, policy)
    if result["exit_code"] != 0 or result["status"] == "timeout":
        if artifact_store is not None:
            artifact_store.capture(execution_context, result, {execution_context["pages"][0]: output},
                                   page=execution_context["pages"][0])
        return result
    elapsed = result["elapsed_ns"]
    try:
        data = read_bounded_regular(output, policy["maximum_output_bytes"])
        actual = normalize(data)
        result.update(verify_pixels(actual, expected, tolerance))
        result.update(pixels_sha256=sha(actual[2]), output_sha256=sha(data),
                      width=actual[0], height=actual[1])
        if expected is not None and actual[:2] == expected[:2] and actual != expected:
            result["different_channels"] = sum(a != b for a, b in zip(actual[2], expected[2]))
            result["max_channel_error"] = max(abs(a - b) for a, b in zip(actual[2], expected[2]))
    except (OSError, ValueError) as error:
        result.update(status="invalid-output", diagnostic=str(error))
    if page is not None and result["status"] in ("ok", "unverified", "incorrect"):
        try:
            stages = parse_stages(diagnostic, page, elapsed)
            if stages is None and stages_required:
                raise ValueError("missing stage timings")
            result["stages"] = stages
            result["stages_status"] = "available" if stages is not None else "unavailable"
        except ValueError as error:
            result.update(status="invalid-timings", diagnostic=str(error))
    if image_sink is not None and result["status"] in ("ok", "unverified", "incorrect"):
        image_sink.append(actual)
    if artifact_store is not None:
        artifact_store.capture(execution_context, result, {execution_context["pages"][0]: output},
                               page=execution_context["pages"][0])
    return result


def preserve_failure(output, result, policy, label):
    """Retain bounded raw failure output locally, with no input-controlled path parts."""
    if result["status"] in ("ok", "unverified"):
        return
    try:
        if output.is_file() and output.stat().st_size <= policy["maximum_output_bytes"]:
            directory = ROOT / "benchmarks/results/local"
            directory.mkdir(parents=True, exist_ok=True)
            artifact = directory / ("failure-" + sha(label.encode()) + ".raw")
            shutil.copyfile(output, artifact)
            result["artifact"] = str(artifact)
    except OSError as error:
        result["artifact_error"] = str(error)

