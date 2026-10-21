#!/usr/bin/env python3
"""One-shot local contention monitor; no retries and no benchmark-result reads.

Run inside the existing exclusive runner: runner -> this guard -> benchmark.
The 5-second process scan detects known jobs; it does not prevent foreign jobs
or guarantee detection of jobs that start and finish between scans. The 90-minute
deadline requests cancellation; owned benchmark cleanup must finish before exit.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import threading
import time

POLL_SECONDS = 5
WINDOW_SECONDS = 90 * 60
CARGO_WORK = {"build", "test", "clippy", "doc", "bench", "check", "run"}
RENDER_PROGRAMS = {"scene_index_profile", "render_profile", "renderbench", "render_bench",
                   "pagura-hayro-bench", "pagura-quartz-bench", "pagura-host-bench"}


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def snapshot():
    # lstart has five whitespace-separated fields under the fixed C locale.
    result = subprocess.run(["ps", "-ww", "-axo", "pid=,ppid=,lstart=,command="],
                            capture_output=True, text=True, timeout=POLL_SECONDS,
                            check=True, env={**os.environ, "LC_ALL": "C"})
    rows = {}
    for line in result.stdout.splitlines():
        fields = line.split(None, 7)
        if len(fields) != 8:
            raise ValueError("incomplete process snapshot")
        pid, ppid = int(fields[0]), int(fields[1])
        rows[pid] = {"pid": pid, "ppid": ppid, "started_local": " ".join(fields[2:7]),
                     "command": fields[7]}
    if os.getpid() not in rows:
        raise ValueError("guard missing from process snapshot")
    return result.stdout, rows


def excluded_processes(rows, child_pid):
    # Exclude our ancestors individually, not every descendant of an ancestor:
    # that would accidentally exclude unrelated sibling agents/builds.
    excluded = {os.getpid()}
    current = os.getpid()
    while current in rows and rows[current]["ppid"] not in excluded:
        current = rows[current]["ppid"]
        excluded.add(current)
    descendants = {os.getpid()}
    if child_pid is not None:
        descendants.add(child_pid)
    while True:
        found = {pid for pid, row in rows.items() if row["ppid"] in descendants}
        if found <= descendants:
            break
        descendants |= found
    return excluded | descendants


def prohibited(command):
    try:
        arguments = shlex.split(command)
    except ValueError:
        arguments = command.split()
    if not arguments:
        return None
    program = Path(arguments[0]).name
    # Also catch directly launched Pagura test/benchmark binaries whose names
    # are not in the fixed list (for example the observed tiles driver).
    executable = arguments[0]
    if "pagura" in executable and "/target" in executable and any(
            part in executable for part in ("/release/", "/debug/")):
        return "pagura-built-program"
    if "pagura" in executable and (program == "warm" or program.startswith("warm-")):
        return "pagura-scratch-render-benchmark"
    if program == "node" and any(arg.endswith("/tools/host_test.mjs") or arg == "tools/host_test.mjs" for arg in arguments[1:]):
        return "pagura-runtime"
    if program == "wasmtime" and any("pagura" in arg and arg.endswith(".wasm") for arg in arguments[1:]):
        return "pagura-runtime"
    if program == "rustc":
        return "rustc"
    if program == "cargo" and any(arg in CARGO_WORK for arg in arguments[1:]):
        return "cargo-compute"
    if program in RENDER_PROGRAMS:
        return "renderer-benchmark"
    if program == "pagura" and any(arg in {"render", "convert", "microbench"} for arg in arguments[1:]):
        return "pagura-render"
    # Relative tools/check.py cannot identify its repository from ps alone;
    # conservatively refuse it too, rather than claiming it is known Pagura.
    scripts = ("tools/check.py", "tools/bench.py", "tools/profile.sh", "tools/conformance.py",
               "tools/scoreboard.py", "benchmarks/scene_index.py", "benchmarks/clip_viewport.py", "benchmarks/glyph_memo.py")
    if any(arg == name or arg.endswith("/" + name) for arg in arguments for name in scripts):
        return "check-or-render-benchmark-script"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", type=Path, required=True, help="new private guard evidence directory")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("one benchmark command is required after --")
    directory = args.evidence_dir.resolve()
    directory.mkdir(parents=True, mode=0o700, exist_ok=False)
    started = time.monotonic()
    deadline = started + WINDOW_SECONDS
    child, cancelled, expired = None, None, False
    watchdog, watchdog_started = None, False
    scan_count = 0
    record = {"schema": 1, "started_utc": utc_now(), "command": command,
              "window_seconds": WINDOW_SECONDS, "poll_seconds": POLL_SECONDS,
              "status": "starting", "events": [], "observation_only": False,
              "limitation": "known-process detection; short jobs between scans may be missed"}

    def save():
        record["scans"] = scan_count
        path = directory / "guard.json"
        with path.open("w") as output:
            json.dump(record, output, indent=2)
            output.write("\n")
        path.chmod(0o600)

    def signal_child(signum):
        if child is not None and child.poll() is None:
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass

    def cancel(signum, _frame):
        nonlocal cancelled
        if cancelled is None:
            cancelled = signum
        signal_child(signum)

    def expire():
        nonlocal expired
        expired = True
        signal_child(signal.SIGTERM)

    def check(stage, retain=False):
        nonlocal scan_count
        captured = utc_now()
        raw, rows = snapshot()
        scan_count += 1
        excluded = excluded_processes(rows, None if child is None else child.pid)
        matches = [{**row, "reason": reason} for pid, row in rows.items()
                   if pid not in excluded and (reason := prohibited(row["command"]))]
        if matches or retain:
            name = f"snapshot-{scan_count:04d}.txt"
            path = directory / name
            path.write_text(raw)
            path.chmod(0o600)
            record["events"].append({"stage": stage, "captured_utc": captured,
                                     "snapshot": name, "matches": matches})
            save()
        return matches  # Stop this fixed campaign if known foreign compute appears.

    previous = {signum: signal.getsignal(signum) for signum in (signal.SIGTERM, signal.SIGINT)}
    exit_code = 70
    try:
        for signum in previous:
            signal.signal(signum, cancel)
        watchdog = threading.Timer(max(0, deadline - time.monotonic()), expire)
        watchdog.daemon = True
        watchdog.start()
        watchdog_started = True
        if check("preflight", retain=True):
            record["status"] = "refused-external-contention"
            exit_code = 75
        elif cancelled is not None:
            record["status"] = "cancelled-before-launch"
            exit_code = 128 + cancelled
        elif expired or time.monotonic() >= deadline:
            record["status"] = "window-expired-before-launch"
            exit_code = 124
        else:
            child = subprocess.Popen(command, start_new_session=True)
            record.update(status="running", child_pid=child.pid, launched_utc=utc_now())
            save()
            if cancelled is not None:
                signal_child(cancelled)
            elif expired:
                signal_child(signal.SIGTERM)
            next_scan = time.monotonic()  # Also closes most of the preflight/spawn gap.
            while child.poll() is None:
                if cancelled is not None:
                    record["status"] = "cancelled"
                    exit_code = 128 + cancelled
                    break
                if expired or time.monotonic() >= deadline:
                    record["status"] = "window-expired"
                    exit_code = 124
                    break
                if time.monotonic() >= next_scan:
                    if check("running"):
                        record["status"] = "invalid-external-contention"
                        exit_code = 143
                        break
                    next_scan = time.monotonic() + POLL_SECONDS
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            if record["status"] == "running":
                if cancelled is not None:
                    record["status"] = "cancelled"
                    exit_code = 128 + cancelled
                elif expired or time.monotonic() >= deadline:
                    record["status"] = "window-expired"
                    exit_code = 124
                elif check("postflight", retain=True):
                    record["status"] = "invalid-external-contention"
                    exit_code = 143
                else:
                    code = child.returncode
                    record["status"] = "completed" if code == 0 else "child-failed"
                    exit_code = code if code >= 0 else 128 - code
    except BaseException as error:
        record.update(status="guard-error", error=f"{type(error).__name__}: {error}")
        exit_code = 128 + cancelled if cancelled is not None else 70
    finally:
        try:
            if watchdog_started:
                watchdog.cancel()
                watchdog.join()
            if child is not None:
                if child.poll() is None:
                    record["cancellation_requested_utc"] = utc_now()
                    signal_child(cancelled if cancelled is not None else signal.SIGTERM)
                    # Never signal foreign jobs or escalate to SIGKILL. The
                    # benchmark forwards cancellation to its owned renderers,
                    # reaps them and stops before any following sample.
                record["child_exit_code"] = child.wait()
            if cancelled is not None:
                record["status"] = "cancelled"
                exit_code = 128 + cancelled
            elif (expired or time.monotonic() >= deadline) and record["status"] == "completed":
                record["status"] = "window-expired"
                exit_code = 124
            record.update(finished_utc=utc_now(), elapsed_seconds=time.monotonic() - started,
                          exit_code=exit_code)
            save()
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)
    print(f"contention guard: {record['status']}; evidence: {directory}", flush=True)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
