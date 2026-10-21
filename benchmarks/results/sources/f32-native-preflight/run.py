#!/usr/bin/env python3
"""Exactly 112 native output checks; raw process metrics are not timing evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sys

sys.dont_write_bytecode = True


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "bindings", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--bindings-sha256", required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    sys.path[:0] = [str(root / "benchmarks"), str(root / "tools")]
    import f32_painted_interval as experiment
    from paired_render import ROOT, MAX_BYTES, MAX_RETAINED, expected_image, source_identity, sha
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    from benchkit.measurement import MeasurementCancelled, run_process
    require(ROOT == root, "wrong helper checkout")
    raw_bindings = read_bounded_regular(args.bindings, MAX_BYTES)
    require(sha(raw_bindings) == args.bindings_sha256, "binding pin mismatch")
    bindings = json.loads(raw_bindings)
    plan = experiment.frozen_plan()
    require(bindings["experiment"] == "f32-painted-interval"
            and bindings["stage"] == "preflight-inputs", "wrong preflight binding stage")
    require(os.environ.get("PAGURA_RUN_ID") and os.environ.get("PAGURA_RUN_RECEIPT"), "owned runner receipt required")
    require(len(set(experiment.CASES)) == 20 and len(set(experiment.CELLS)) == 56
            and len(experiment.CELLS) == 56, "wrong fixed matrix")
    require([(c["case"], c["mode"], c["operation"]) for c in plan["cells"]] == list(experiment.CELLS), "plan matrix differs")
    identities = bindings["identities"]
    require(identities["candidate"]["root"] == str(root), "wrong candidate root")
    experiment.require_baseline(identities["baseline"]["root"])
    def verify_identities():
        for side in ("baseline", "candidate"):
            item = identities[side]
            require(all(item[k] == plan["identities_to_freeze"][side][k] for k in
                        ("root", "commit", "source_sha256", "binary", "binary_sha256")), "plan identity mismatch")
            require(source_identity(item["root"]) == {k: item[k] for k in ("root", "commit", "source_sha256")}
                    and hash_regular_file(item["binary"]) == item["binary_sha256"], "source/binary changed")
    verify_identities()
    pins = {str(Path(__file__).resolve()): hash_regular_file(Path(__file__)),
            str(args.bindings.resolve()): args.bindings_sha256}
    for relative, expected in bindings["bound_files_sha256"].items():
        path = (root / relative).resolve()
        require(path.is_relative_to(root) and hash_regular_file(path) == expected, "frozen helper changed")
        pins[str(path)] = expected
    def modules():
        return {str(Path(m.__file__).resolve()): hash_regular_file(m.__file__) for m in tuple(sys.modules.values())
                if getattr(m, "__file__", None) and Path(m.__file__).resolve().is_relative_to(root)}
    loaded = modules()
    require(all(str(Path(p).relative_to(root)) in bindings["bound_files_sha256"] for p in loaded
                if Path(p).is_relative_to(root / "benchmarks")), "fixture helper omitted from frozen bindings")
    pins.update(loaded)
    for path in (Path(sys.executable), Path(sys.orig_argv[0])):
        pins[str(path.resolve(strict=True))] = hash_regular_file(path)
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "experiment": "f32-painted-interval", "validation": "native-preflight",
              "status": "running", "admission": False, "expected_outputs": 112, "all_correct": False,
              "bindings_sha256": args.bindings_sha256, "identities": identities, "pins": pins,
              "argv": sys.orig_argv, "run_id": os.environ.get("PAGURA_RUN_ID"),
              "run_receipt": os.environ.get("PAGURA_RUN_RECEIPT"), "inputs": {}, "records": [],
              "limitation": "Untimed correctness only; final build/receipt/guard authentication remains coordinator-owned."}
    stored, artifacts = 0, {}
    def retain(relative, data):
        nonlocal stored
        path = directory / relative
        if str(path) not in artifacts:
            require(stored + len(data) <= MAX_RETAINED, "retention budget exhausted")
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as handle:
                handle.write(data)
            stored += len(data)
            artifacts[str(path)] = {"path": str(path), "sha256": sha(data), "bytes": len(data)}
        require(artifacts[str(path)]["sha256"] == sha(data), "artifact collision")
        return artifacts[str(path)]
    def checkpoint():
        report.update(stored_bytes=stored, artifacts=list(artifacts.values()))
        temporary = directory / "report.pending.json"
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(directory / "report.json")
    def cancelled(number, _frame):
        report.update(status="incomplete", all_correct=False)
        raise MeasurementCancelled(number)
    previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    try:
        for s in previous:
            signal.signal(s, cancelled)
        checkpoint()
        retain("bindings.json", raw_bindings)
        oracles = {}
        for name in experiment.CASES:
            pdf, oracle = experiment.fixture(name)
            oracles[name] = oracle
            report["inputs"][name] = {"pdf": retain(f"inputs/{name}.pdf", pdf), "oracle": retain(f"oracles/{name}.ppm", oracle)}
        for index, (name, mode, operation) in enumerate(experiment.CELLS):
            width, height, rgb = expected_image(oracles[name], operation)
            pair = []
            for side in ("baseline", "candidate"):
                item, inputs = identities[side], report["inputs"][name]
                require(hash_regular_file(item["binary"]) == item["binary_sha256"] and
                        all(hash_regular_file(a["path"]) == a["sha256"] for a in inputs.values()), "invocation input/binary changed")
                output = directory / f"pending-{index:02}-{side}.rgba"
                command = [item["binary"], inputs["pdf"]["path"], mode, operation, str(output), "72"]
                row = {"cell": index, "case": name, "mode": mode, "operation": operation, "side": side, "command": command, "verified": False}
                report["records"].append(row)
                checkpoint()
                try:
                    process, log = run_process(command, {"timeout_seconds": 120, "measure_rss": False}, log_budget=2**20)
                    row.update(process=process, log=retain(f"logs/{index:02}-{side}.log", log.encode()))
                finally:
                    if output.exists():
                        raw = read_bounded_regular(output, MAX_BYTES)
                        row["output"] = retain("blobs/" + sha(raw) + ".rgba", raw)
                        output.unlink()
                    checkpoint()
                try:
                    require(len(log.encode()) < 2**20 and "\ufffd" not in log and "PAGURA_N16_REACH_V" not in log, "invalid/instrumented log")
                    require(type(process.get("exit_code")) is int and process["exit_code"] == 0 and process.get("status") != "timeout", "renderer failed")
                    require("output" in row, "missing output")
                    header = f"RGBA8 {width} {height} premultiplied\n".encode()
                    require(raw.startswith(header) and len(raw) == len(header) + width * height * 4, "raw RGBA format/extent")
                    pixels = raw[len(header):]
                    require(pixels[3::4] == b"\xff" * (width * height) and all(pixels[c::4] == rgb[c::3] for c in range(3)), "independent RGB/alpha mismatch")
                    stages = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
                    require(len(stages) == 1 and all(stages[0].get(k) == v for k, v in
                            {"schema": 1, "accuracy": mode, "operation": operation, "dpi": 72, "width": width, "height": height}.items()), "request identity mismatch")
                    row["verified"] = True
                except (ValueError, TypeError, AttributeError) as error:
                    row["error"] = str(error)
                pair.append(row)
                checkpoint()
            if all(row["verified"] for row in pair):
                require(pair[0]["output"]["sha256"] == pair[1]["output"]["sha256"], "same-mode raw parity mismatch")
        verify_identities()
        require(modules() == loaded and all(hash_regular_file(p) == h for p, h in pins.items()), "helper/runtime/binding changed")
        require(all(hash_regular_file(a["path"]) == a["sha256"] for a in artifacts.values()), "retained artifact changed")
        require(len(report["records"]) == 112, "incomplete fixed matrix")
        correct = all(row["verified"] for row in report["records"])
        report.update(status="verified-native-preflight" if correct else "failed", all_correct=correct, completed_outputs=112, identities_unchanged=True)
    except BaseException as error:
        report.update(status="incomplete", all_correct=False, fatal_error=f"{type(error).__name__}: {error}")
        raise
    finally:
        try:
            checkpoint()
        finally:
            for s, handler in previous.items():
                signal.signal(s, handler)
    print(directory / "report.json", flush=True)
    return 0 if report["all_correct"] else 2


if __name__ == "__main__":
    sys.exit(main())
