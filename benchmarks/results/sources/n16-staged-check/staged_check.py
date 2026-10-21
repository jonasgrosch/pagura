#!/usr/bin/env python3
"""Run one frozen authoritative check stage; aggregate all retained attempts."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
LIMIT, LOG_LIMIT = 64 << 20, 16 << 20
ENV_NAMES = set("PATH HOME USER SHELL TMPDIR LANG LC_ALL LC_CTYPE TZ CC CXX AR AS LD RANLIB SDKROOT MACOSX_DEPLOYMENT_TARGET DEVELOPER_DIR PYTHONHASHSEED PYTHONPATH PYTHONHOME PYTHONDONTWRITEBYTECODE".split())


def require(value, message):
    if not value:
        raise ValueError(message)


def environment():
    return {k: v for k, v in os.environ.items() if k in ENV_NAMES or k.startswith(("CARGO_", "RUST", "NODE_", "LC_"))}


def load(path):
    return json.loads(read(path, LIMIT))


def write(path, value, *, new=False):
    data = json.dumps(value, indent=2, allow_nan=False) + "\n"
    if new:
        with path.open("x") as handle:
            handle.write(data)
    else:
        pending = path.with_suffix(".pending")
        pending.write_text(data)
        pending.replace(path)


def snapshot(root):
    return {"commit": subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip(), "sha256": source_digest()}


def extra_paths(root):
    paths = {p.resolve() for name in ("web", "corpus/generated", ".cargo") for p in (root / name).rglob("*")
             if p.is_file() and "__pycache__" not in p.parts}
    homes = [Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")), *[p / ".cargo" for p in (root, *root.parents)]]
    paths.update((home / name).resolve() for home in homes for name in ("config", "config.toml") if (home / name).is_file())
    return paths


def planned(root):
    spec = importlib.util.spec_from_file_location("frozen_check", root / "tools/check.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    result = [{"argv": argv, "env": env} for argv, env in module.planned_commands(jobs=1)]
    require(len(result) == 14, "authoritative command count changed; explicit re-plan required")
    return result


def freeze(args, root):
    require(not args.plan.exists() and not (args.plan.parent / "attempts").exists(), "freeze destination already exists")
    before, commands = snapshot(root), planned(root)
    paths = extra_paths(root) | {Path(__file__).resolve(), root / "tools/check.py", args.guard.resolve()}
    paths.update(Path(module.__file__).resolve() for module in tuple(sys.modules.values())
                 if getattr(module, "__file__", None) and Path(module.__file__).resolve().is_relative_to(root))
    for name in ("cargo", "rustc", "rustup", "node"):
        selected = shutil.which(name)
        require(selected, "runtime unavailable: " + name)
        paths.add(Path(selected).resolve())
    for name in ("cargo", "rustc"):
        paths.add(Path(subprocess.check_output([shutil.which("rustup"), "which", name], text=True).strip()).resolve(strict=True))
    paths.update((Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve()))
    pins = {str(path): sha(path) for path in sorted(paths)}
    require(snapshot(root) == before, "source changed while freezing")
    plan = {"schema": 1, "kind": "staged-authoritative-check", "root": str(root), "source": before,
            "commands": commands, "environment": environment(), "files_sha256": pins,
            "extra_paths": sorted(map(str, extra_paths(root))), "python": str(Path(sys.orig_argv[0]).resolve()),
            "python_executable": str(Path(sys.executable).resolve()),
            "script": str(Path(__file__).resolve()), "guard": str(args.guard.resolve()),
            "attempts": str(args.plan.parent / "attempts"), "timeout_seconds": 1200,
            "log_limit": LOG_LIMIT, "retry_policy": "external-cancellation-only; semantic failure blocks this freeze"}
    args.plan.parent.mkdir(parents=True, exist_ok=True)
    write(args.plan, plan, new=True)
    Path(plan["attempts"]).mkdir()
    print(sha(args.plan))


def verify(plan, root):
    require(plan["schema"] == 1 and plan["root"] == str(root) and plan["source"] == snapshot(root), "source/freeze drift")
    require(plan["commands"] == planned(root) and plan["environment"] == environment(), "command/environment drift")
    require(plan["extra_paths"] == sorted(map(str, extra_paths(root))), "fixture/web/config inventory drift")
    require(all(sha(path) == digest for path, digest in plan["files_sha256"].items()), "helper/runtime/fixture/config drift")
    require(plan["script"] == str(Path(__file__).resolve()) and plan["python"] == str(Path(sys.orig_argv[0]).resolve())
            and plan["python_executable"] == str(Path(sys.executable).resolve()), "coordinator/runtime drift")


def command(plan, plan_path, digest, index, name):
    return [plan["python"], plan["script"], "stage", "--root", plan["root"], "--plan", str(plan_path),
            "--plan-sha256", digest, "--index", str(index), "--attempt", name]


def stage(args, plan, root):
    require(type(args.index) is int and 0 <= args.index < 14 and isinstance(args.attempt, str) and args.attempt.isidentifier(), "invalid stage/attempt")
    attempt = Path(plan["attempts"]) / args.attempt
    directory = attempt / "stage"
    directory.mkdir(parents=True, exist_ok=False)
    report_path = directory / "report.json"
    report = {"schema": 1, "index": args.index, "plan_sha256": args.plan_sha256, "status": "incomplete",
              "command": plan["commands"][args.index], "producer_argv": sys.orig_argv,
              "run_id": os.environ.get("PAGURA_RUN_ID"), "receipt": os.environ.get("PAGURA_RUN_RECEIPT"), "log": None}
    old = {key: os.environ.get(key) for key in report["command"]["env"]}
    def cancel(signum, _frame):
        report.update(status="cancelled", cancellation_signal=signum)
        raise MeasurementCancelled(signum)
    handlers = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
    try:
        for signum in handlers:
            signal.signal(signum, cancel)
        write(report_path, report)
        verify(plan, root)
        report["source_before"] = snapshot(root)
        require(sys.orig_argv == command(plan, args.plan, args.plan_sha256, args.index, args.attempt), "unexpected actual stage argv")
        require(report["run_id"] and report["receipt"] == str(attempt / "receipt.json"), "owned receipt path required")
        live = load(report["receipt"])
        require(live["status"] == "running" and live["run_id"] == report["run_id"] and live["source_before"] == plan["source"], "live runner mismatch")
        os.environ.update(report["command"]["env"])
        write(report_path, report)
        process, log = run_process(report["command"]["argv"], {"timeout_seconds": plan["timeout_seconds"], "measure_rss": False}, log_budget=LOG_LIMIT)
        report["process"] = process
        with (directory / "stage.log").open("x") as handle:
            handle.write(log)
        report["log"] = {"path": str(directory / "stage.log"), "sha256": sha(directory / "stage.log")}
        require(len(log.encode()) < LOG_LIMIT and "\ufffd" not in log, "truncated/non-UTF8 stage log")
        require(type(process.get("exit_code")) is int and process["exit_code"] == 0 and process.get("status") != "timeout", "stage command failed")
        report["status"] = "command-passed"
    except BaseException as error:
        report["status"] = "cancelled" if isinstance(error, MeasurementCancelled) else "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        if report["log"] is None:
            report["log_unavailable"] = "run_process did not return its captured log"
        raise
    finally:
        for key, value in old.items():
            os.environ.pop(key, None) if value is None else os.environ.__setitem__(key, value)
        try:
            report["source_after"] = snapshot(root)
            try:
                verify(plan, root)
            except BaseException as error:
                report.update(status="failed", final_identity_error=str(error))
                raise
        finally:
            try:
                write(report_path, report)
            finally:
                for signum, handler in handlers.items():
                    signal.signal(signum, handler)


def successful(receipt):
    return (receipt.get("status") == "completed" and receipt.get("source_unchanged") is True
            and receipt.get("source_before") == receipt.get("source_after") and not receipt.get("errors")
            and not receipt.get("artifact_errors") and receipt.get("cancellation_signal") is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0 for k in ("exit_code", "wrapper_exit_code")))


def decide(plan, attempts):
    accepted, prior_end = {}, None
    for item in sorted(attempts, key=lambda row: row["receipt"]["started_at"]):
        index, report, receipt, guard = (item[k] for k in ("index", "report", "receipt", "guard"))
        require(receipt["source_before"] == receipt["source_after"] == plan["source"], "attempt source drift")
        require(index == len(accepted) and (prior_end is None or prior_end <= receipt["started_at"]), "stage repeated/out of order/overlapped")
        prior_end = receipt["ended_at"]
        if report and (report.get("status") == "failed" or report.get("process", {}).get("status") == "timeout"):
            raise ValueError("semantic/timeout/evidence failure blocks this freeze")
        if report and "process" in report:
            require(type(report["process"].get("exit_code")) is int and report["process"]["exit_code"] == 0, "nonzero stage result blocks this freeze")
        if successful(receipt):
            require(report and report["status"] == "command-passed" and report["source_before"] == report["source_after"] == plan["source"], "stage incomplete")
            require(report["command"] == plan["commands"][index] and report["run_id"] == receipt["run_id"], "stage command/run drift")
            require(report.get("log") and type(report.get("process", {}).get("exit_code")) is int
                    and report["process"]["exit_code"] == 0, "missing successful process/log")
            require(guard.get("status") == "completed" and guard.get("observation_only") is False
                    and all(type(guard.get(k)) is int and guard[k] == 0 for k in ("exit_code", "child_exit_code"))
                    and not guard.get("cancellation_requested_utc") and guard.get("events")
                    and all(e.get("matches") == [] for e in guard["events"])
                    and {"preflight", "postflight"} <= {e.get("stage") for e in guard["events"]}, "observer incomplete/contention")
            accepted[index] = receipt
        else:
            foreign = guard.get("status") in ("refused-external-contention", "invalid-external-contention") and any(e.get("matches") for e in guard.get("events", []))
            cancelled = guard.get("status") == "cancelled" and receipt.get("cancellation_signal") in (signal.SIGINT, signal.SIGTERM)
            require((foreign or cancelled) and receipt.get("status") in ("failed", "canceled")
                    and not receipt.get("errors") and (report is None or report.get("status") in ("cancelled", "command-passed")), "unresolved failed attempt")
            allowed_missing = {str(Path(report["receipt"]).parent / "stage/stage.log")} if report else set(item["missing_stage_paths"])
            require(set(receipt.get("artifact_errors", {})) <= allowed_missing, "unexpected missing attempt evidence")
    require(set(accepted) == set(range(14)), "missing authoritative stage")


def aggregate(args, plan, root):
    verify(plan, root)
    base = Path(plan["attempts"])
    inventory = {str(p): sha(p) for p in sorted(base.rglob("*")) if p.is_file()}
    attempts = []
    for directory in sorted(base.iterdir()):
        require(directory.is_dir(), "unexpected attempts inventory entry")
        receipt, guard = load(directory / "receipt.json"), load(directory / "observer/guard.json")
        argv = guard["command"]
        index = int(argv[argv.index("--index") + 1])
        expected = command(plan, args.plan, args.plan_sha256, index, directory.name)
        require(0 <= index < 14 and argv == expected and receipt["argv"] == [plan["python"], plan["guard"], "--evidence-dir", str(directory / "observer"), "--", *expected], "attempt launch chain differs")
        require(receipt["source_root"] == str(root) and receipt["cwd"] == str(root), "attempt root differs")
        report_path = directory / "stage/report.json"
        report = load(report_path) if report_path.exists() else None
        for event in guard.get("events", []):
            name = event.get("snapshot")
            require(isinstance(name, str) and Path(name).name == name and read(directory / "observer" / name, LIMIT), "missing/unsafe raw observer snapshot")
            raw_path = str(directory / "observer" / name)
            require(raw_path not in receipt["artifacts"] or receipt["artifacts"][raw_path] == sha(raw_path), "original observer snapshot pin differs")
        required = [directory / "observer/guard.json"] + ([report_path] if report else [])
        if report:
            require(report["index"] == index and report["plan_sha256"] == args.plan_sha256 and report["producer_argv"] == expected and report["receipt"] == str(directory / "receipt.json"), "report/freeze identity differs")
            if report.get("log"):
                require(report["log"]["path"] == str(directory / "stage/stage.log") and sha(report["log"]["path"]) == report["log"]["sha256"], "stage log mismatch")
                required.append(Path(report["log"]["path"]))
        require(all(receipt["artifacts"].get(str(p)) == sha(p) for p in required), "final receipt lacks exact stage artifacts")
        attempts.append({"attempt": directory.name, "index": index, "report": report, "receipt": receipt, "guard": guard,
                         "missing_stage_paths": [str(report_path), str(directory / "stage/stage.log")]})
    decide(plan, attempts)
    verify(plan, root)
    require(inventory == {str(p): sha(p) for p in sorted(base.rglob("*")) if p.is_file()}, "attempt evidence changed")
    write(args.output, {"schema": 1, "status": "verified-staged-check", "admission": False, "source": plan["source"],
          "plan_sha256": args.plan_sha256, "stage_count": 14, "attempts": attempts, "artifacts_sha256": inventory,
          "scope": "All exact authoritative commands passed serially on one freeze; separate attempts, not one uninterrupted check.py process.",
          "snapshot_binding": "Observer snapshots are read and pinned by this aggregate; the existing guard records names without hashes. Original receipt pins are additionally checked wherever present."}, new=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("freeze", "stage", "aggregate"))
    for name in ("root", "plan", "guard", "output"):
        parser.add_argument("--" + name, type=Path, required=name in ("root", "plan"))
    parser.add_argument("--plan-sha256"); parser.add_argument("--index", type=int); parser.add_argument("--attempt")
    args = parser.parse_args()
    args.root, args.plan = args.root.resolve(strict=True), args.plan.resolve()
    require(Path.cwd().resolve() == args.root, "run from the frozen candidate root")
    sys.path.insert(0, str(args.root / "tools"))
    global read, sha, source_digest, run_process, MeasurementCancelled
    from benchkit.artifacts import read_bounded_regular as read, hash_regular_file as sha
    from benchkit.config import ROOT, source_digest
    from benchkit.measurement import run_process, MeasurementCancelled
    require(ROOT == args.root, "imported helper root differs")
    if args.action == "freeze":
        require(args.guard is not None, "guard required")
        freeze(args, args.root)
    else:
        require(sha(args.plan) == args.plan_sha256, "external plan pin differs")
        plan = load(args.plan)
        stage(args, plan, args.root) if args.action == "stage" else aggregate(args, plan, args.root)


if __name__ == "__main__":
    main()
