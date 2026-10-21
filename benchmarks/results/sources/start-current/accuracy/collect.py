#!/usr/bin/env python3
"""Collect 108 fixed untimed native accuracy probes; no performance admission."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sys

import references

MAX_FILE, MAX_TOTAL, LOG_BUDGET = 4 << 20, 64 << 20, 64 << 10


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--bindings", type=Path, required=True)
    parser.add_argument("--bindings-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root, directory = args.root.resolve(), args.output.resolve()
    if Path.cwd().resolve() != root:
        parser.error("run from the candidate checkout root")
    if not os.environ.get("PAGURA_RUN_ID") or not os.environ.get("PAGURA_RUN_RECEIPT"):
        parser.error("the serial coordinator's run ID and receipt are required")
    sys.path.insert(0, str(root / "benchmarks"))
    from paired_render import source_identity
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    from benchkit.measurement import MeasurementCancelled, run_process

    def read(path):
        return read_bounded_regular(Path(path), MAX_FILE)

    def pinned(path, digest):
        raw = read(path)
        if sha(raw) != digest:
            raise ValueError("frozen file changed: " + str(path))
        return raw

    bindings_raw = pinned(args.bindings, args.bindings_sha256)
    bindings = json.loads(bindings_raw)
    identities = bindings["identities"]
    if (bindings.get("schema") != 1 or bindings.get("experiment") != references.PLAN["experiment"]
            or bindings.get("plan") != references.PLAN or set(identities) != set(references.SIDES)
            or Path(identities["candidate"]["root"]).resolve() != root
            or not bindings.get("completed_evidence")):
        raise ValueError("invalid fixed bindings")
    here = Path(__file__).resolve().parent
    required = (Path(__file__).resolve(), Path(references.__file__).resolve(), here / "accuracy_probe.rs",
                here / "baseline/Cargo.toml", here / "candidate/Cargo.toml", root / "benchmarks/paired_render.py")
    harness = bindings["harness_sha256"]
    if not {str(path) for path in required} <= set(harness):
        raise ValueError("missing required harness pins")

    def check_sources():
        for identity in identities.values():
            if source_identity(identity["root"]) != {key: identity[key] for key in ("root", "commit", "source_sha256")}:
                raise ValueError("product source identity changed")
            if hash_regular_file(Path(identity["binary"])) != identity["binary_sha256"]:
                raise ValueError("accuracy binary changed")
        for name, digest in harness.items():
            if hash_regular_file(Path(name)) != digest:
                raise ValueError("harness changed: " + name)

    check_sources()
    requests = references.probes(root, pinned)
    directory.mkdir(parents=True, exist_ok=False)
    artifacts, records, inputs = {}, [], []
    stored = 0
    report = {"schema": 1, "experiment": references.PLAN["experiment"], "plan": references.PLAN,
              "status": "incomplete", "candidate_quality_verified": False, "admission": False,
              "identities": identities, "bindings_sha256": sha(bindings_raw), "harness_sha256": harness,
              "run_id": os.environ["PAGURA_RUN_ID"], "receipt": os.environ["PAGURA_RUN_RECEIPT"],
              "producer_argv": list(sys.orig_argv), "records": records, "inputs": inputs, "artifacts": artifacts,
              "limitations": ["Untimed native public compile_page and retained-list render; no speed, RSS or release claim",
                              "Declared scalar coverage-byte reference is distinct from continuous scene coverage",
                              "Original baseline errors remain errors; candidate output never supplies a reference",
                              "Root precision repeats 16 separately collected untimed correctness outputs; no performance pooling",
                              "Final outer receipt and observer authentication is required separately; live snapshots are not terminal proof"]}

    def retain(name, raw):
        nonlocal stored
        if name in artifacts or stored + len(raw) > MAX_TOTAL:
            raise ValueError("duplicate artifact or retained byte limit")
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)
        artifacts[name] = {"sha256": sha(raw), "bytes": len(raw)}
        stored += len(raw)
        return name

    def run(request, index, mode, side):
        nonlocal stored
        input_row = inputs[index]
        if read(directory / input_row["pdf"]) != request["pdf"]:
            raise ValueError("retained input changed")
        identity = identities[side]
        if hash_regular_file(Path(identity["binary"])) != identity["binary_sha256"]:
            raise ValueError("binary changed before invocation")
        name = f"outputs/{index:02d}-{mode}-{side}.rgba"
        output = directory / name
        output.parent.mkdir(exist_ok=True)
        command = [identity["binary"], str(directory / input_row["pdf"]), mode, request["operation"], str(output),
                   *map(str, request["background"]), *map(str, request["tile"])]
        row = {"request": index, "case": request["case"], "mode": mode, "side": side, "command": command,
               "reference": input_row["reference"], "process_ok": False, "independent_pass": False}
        records.append(row)
        raw = None
        try:
            process, log = run_process(command, {"timeout_seconds": 60, "measure_rss": False}, log_budget=LOG_BUDGET)
            row["process"] = process
            row["log"] = retain(f"logs/{index:02d}-{mode}-{side}.txt", log.encode())
        except BaseException as error:
            row["interrupted"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            try:
                raw = read(output)
            except (OSError, ValueError) as error:
                row["output_error"] = str(error)
            if raw is not None:
                if stored + len(raw) > MAX_TOTAL:
                    raise ValueError("retained byte limit")
                artifacts[name] = {"sha256": sha(raw), "bytes": len(raw)}
                stored += len(raw)
                row["output"] = name
        try:
            if len(log.encode()) >= LOG_BUDGET:
                raise ValueError("possibly truncated process log")
            metadata = [json.loads(line) for line in log.splitlines() if line.lstrip().startswith("{")]
            reference = request["reference"]
            expected = {"schema": 1, "accuracy": mode, "operation": request["operation"], "page": 0, "dpi": 72,
                        "background": request["background"], "tile": request["tile"],
                        "width": reference["width"], "height": reference["height"],
                        "diagnostics": 0, "suppressed_diagnostics": 0}
            if metadata != [expected]:
                raise ValueError("adapter invocation or diagnostics record mismatch")
            row["metadata"] = metadata[0]
            row["process_ok"] = type(process.get("exit_code")) is int and process["exit_code"] == 0 and process.get("status") != "timeout"
            if raw is None:
                raise ValueError("missing output")
            row["quality"] = references.evaluate(raw, reference)
            row["independent_pass"] = row["process_ok"] and row["quality"]["independent_pass"]
        except ValueError as error:
            row["error"] = str(error)

    previous = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
    cancelled = False

    def cancel(signum, _frame):
        nonlocal cancelled
        if not cancelled:
            cancelled = True
            raise MeasurementCancelled(signum)

    def write_report():
        report["stored_bytes"] = stored
        temporary = directory / "report.pending"
        temporary.write_bytes(encoded(report))
        temporary.replace(directory / "report.json")

    try:
        for signum in previous:
            signal.signal(signum, cancel)
        write_report()
        report["bindings"] = retain("bindings.json", bindings_raw)
        report["completed_evidence"] = []
        for index, item in enumerate(bindings["completed_evidence"]):
            raw = pinned(item["path"], item["sha256"])
            report["completed_evidence"].append({**item, "artifact": retain(f"evidence/{index:02d}.json", raw)})
        report["receipt_start_snapshot"] = retain("receipt-start-snapshot.json", read(report["receipt"]))
        for index, request in enumerate(requests):
            inputs.append({"request": index, **{key: request[key] for key in ("case", "operation", "background", "tile")},
                           "pdf": retain(f"inputs/{index:02d}.pdf", request["pdf"]),
                           "reference": retain(f"references/{index:02d}.json", encoded(request["reference"]))})
        for index, request in enumerate(requests):
            for mode in references.MODES:
                for side in references.SIDES:
                    run(request, index, mode, side)
        check_sources()
        report["status"] = "completed"
        report["candidate_quality_verified"] = len(records) == 108 and all(row["independent_pass"] for row in records if row["side"] == "candidate")
        report["all_processes_ok"] = len(records) == 108 and all(row["process_ok"] for row in records)
        report["baseline_failed_probes"] = [{key: row[key] for key in ("request", "case", "mode")} for row in records
                                           if row["side"] == "baseline" and not row["independent_pass"]]
    except BaseException as error:
        report["status"] = "incomplete"
        report["candidate_quality_verified"] = False
        report["fatal_error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            write_report()
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)
    return 0 if report["candidate_quality_verified"] and report["all_processes_ok"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
