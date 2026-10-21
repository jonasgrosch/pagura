#!/usr/bin/env python3
"""One fixed start/current collection, retaining quality failures without retry."""
import argparse
import json
import os
import platform
import re
import signal
import sys
import time
import uuid
from pathlib import Path

from protocol import (BASELINE, CANDIDATE, Journal, LOG_BUDGET, MAX_FILE, MAX_RETAINED,
                      PAIRS, RSS_LINE, SEED, SIDES, WARMUPS, cells, classification,
                      encoded, input_key, opaque_bytes, quality, require, schedule, sha)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "bindings", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--bindings-sha256", required=True)
    parser.add_argument("--phase", choices=("preflight", "confirmation"), required=True)
    parser.add_argument("--preflight-pins", type=Path)
    parser.add_argument("--preflight-pins-sha256")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("requires Darwin /usr/bin/time -l byte units")
    if not re.fullmatch(r"[0-9a-f]{64}", args.bindings_sha256):
        parser.error("independent bindings SHA-256 required")
    if ((args.phase == "confirmation") != (args.preflight_pins is not None)
            or (args.preflight_pins is None) != (args.preflight_pins_sha256 is None)):
        parser.error("only confirmation requires both preflight pins arguments")
    root, binding_path, directory = (p.resolve() for p in (args.root, args.bindings, args.output))
    directory.mkdir(parents=True, exist_ok=False)
    journal = Journal(directory / "events.jsonl")
    started, started_unix = time.monotonic_ns(), time.time_ns()
    report = {"schema": 1, "experiment": "start-current", "phase": args.phase, "status": "running",
              "admission": False, "producer_argv": list(sys.orig_argv), "records": [],
              "bindings_sha256": args.bindings_sha256, "started_unix_ns": started_unix,
              "runner": {"run_id": os.environ.get("PAGURA_RUN_ID"),
                         "receipt": os.environ.get("PAGURA_RUN_RECEIPT")}}
    helpers, stored, previous, closure = None, 0, {}, None
    return_code = 1

    def save_report():
        report["stored_bytes"] = stored
        raw = encoded(report)
        require(len(raw) <= 256 << 20, "final report exceeds fixed byte budget")
        pending = directory / "report.pending"
        pending.write_bytes(raw)
        pending.replace(directory / "report.json")

    def retain(relative, raw):
        nonlocal stored
        path = directory / relative
        require(len(raw) <= MAX_FILE, "individual artifact exceeds fixed byte budget")
        if path.exists():
            require(helpers.read_bounded_regular(path, MAX_FILE) == raw, "retained artifact changed")
        else:
            require(stored + len(raw) <= MAX_RETAINED, "retention budget exhausted")
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as handle:
                handle.write(raw)
            path.chmod(0o400)
            stored += len(raw)
        return {"path": relative, "sha256": sha(raw), "bytes": len(raw)}

    def pinned(descriptor, limit=MAX_FILE):
        require(isinstance(descriptor, dict) and isinstance(descriptor.get("path"), str)
                and Path(descriptor["path"]).is_absolute()
                and re.fullmatch(r"[0-9a-f]{64}", descriptor.get("sha256", "")), "invalid external pin")
        raw = helpers.read_bounded_regular(Path(descriptor["path"]), limit)
        require(sha(raw) == descriptor["sha256"], "external evidence changed: " + descriptor["path"])
        if "bytes" in descriptor:
            require(type(descriptor["bytes"]) is int and len(raw) == descriptor["bytes"], "artifact byte count differs")
        return raw

    def completed(evidence, required):
        receipt_raw, guard_raw = pinned(evidence["receipt"]), pinned(evidence["guard"])
        receipt, guard = json.loads(receipt_raw), json.loads(guard_raw)
        require(receipt.get("status") == "completed" and receipt.get("source_unchanged") is True
                and receipt.get("errors") == [] and receipt.get("artifact_errors") == {}
                and all(type(receipt.get(k)) is int and receipt[k] == 0 for k in ("exit_code", "wrapper_exit_code")),
                "prerequisite receipt did not complete successfully")
        for descriptor in (*required, evidence["guard"]):
            require(receipt.get("artifacts", {}).get(descriptor["path"]) == descriptor["sha256"],
                    "prerequisite receipt does not bind required artifact")
        events = guard.get("events")
        require(guard.get("status") == "completed" and guard.get("observation_only") is False
                and guard.get("cancellation_requested_utc") is None
                and isinstance(events, list) and events
                and {"preflight", "postflight"} <= {e.get("stage") for e in events}
                and all(e.get("matches") == [] for e in events), "prerequisite contention guard invalid")
        return receipt_raw, guard_raw

    save_report()
    try:
        sys.dont_write_bytecode = True
        sys.path[:0] = [str(root / "tools"), str(root / "benchmarks")]
        from benchkit import artifacts as helpers
        from benchkit.config import ROOT
        from benchkit.measurement import MeasurementCancelled, rss_wrapper, run_process
        import paired_render
        import opaque_composition as opaque
        import color_reuse_fixtures as analytic
        require(Path(ROOT).resolve() == paired_render.ROOT.resolve() == root
                and rss_wrapper() == ["/usr/bin/time", "-l"], "helper root or RSS collector differs")

        def cancel(signum, _frame):
            raise MeasurementCancelled(signum)

        previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
        for signum in previous:
            signal.signal(signum, cancel)
        pointer = report["runner"]
        require(isinstance(pointer["run_id"], str) and str(uuid.UUID(pointer["run_id"])) == pointer["run_id"]
                and isinstance(pointer["receipt"], str) and Path(pointer["receipt"]).is_absolute(), "runner identity missing")
        start_raw = helpers.read_bounded_regular(Path(pointer["receipt"]), MAX_FILE)
        start = json.loads(start_raw)
        require(type(start.get("schema")) is int and start["schema"] == 1 and start.get("status") == "running"
                and start.get("kind") == "benchmark" and start.get("run_id") == pointer["run_id"]
                and start.get("source_root") == start.get("cwd") == str(root)
                and isinstance(start.get("source_before"), dict), "running receipt differs")
        runner_start = retain("runner-start.json", start_raw)
        journal.append("run-start", runner=pointer, source_before=start["source_before"],
                       producer_argv=report["producer_argv"], started_unix_ns=started_unix)
        binding_raw = pinned({"path": str(binding_path), "sha256": args.bindings_sha256})
        binding = json.loads(binding_raw)
        identities = binding["identities"]
        require(binding.get("experiment") == "start-current" and set(identities) == {"baseline", "candidate"}
                and identities["baseline"]["commit"] == BASELINE and identities["candidate"]["commit"] == CANDIDATE
                and Path(identities["candidate"]["root"]).resolve() == root, "frozen experiment identities differ")
        binaries = {side: Path(identity["binary"]).resolve() for side, identity in identities.items()}
        for side, identity in identities.items():
            require(str(binaries[side]) == identity["binary"]
                    and helpers.hash_regular_file(binaries[side]) == identity["binary_sha256"]
                    and paired_render.source_identity(identity["root"]) ==
                    {k: identity[k] for k in ("root", "commit", "source_sha256")}, "source or binary identity differs")
        retain("bindings.json", binding_raw)
        build_raw = pinned(binding["build_report"])
        build = json.loads(build_raw)
        require(build.get("status") == "built-both-arms" and build.get("identities") == identities,
                "build identities differ")
        for side, identity in identities.items():
            source = {k: identity[k] for k in ("root", "commit", "source_sha256")}
            item = build["builds"][side]
            require(item["source_before"] == item["source_after"] == source
                    and type(item["process"].get("exit_code")) is int and item["process"]["exit_code"] == 0,
                    "build source or completion differs")
        evidence = [e for e in binding["completed_evidence"]
                    if e["artifacts"].get(binding["build_report"]["path"]) == binding["build_report"]["sha256"]]
        require(len(evidence) == 1, "ambiguous build receipt")
        receipt_raw, guard_raw = completed(evidence[0], [binding["build_report"]])
        for name, raw in (("build.json", build_raw), ("build-receipt.json", receipt_raw), ("build-guard.json", guard_raw)):
            retain(name, raw)
        for relative, pin in binding["bound_files_sha256"].items():
            path = (root / relative).resolve()
            require(path.is_relative_to(root) and helpers.hash_regular_file(path) == pin, "bound source differs")
        example = Path("crates/pagura-render/examples/scene_index_profile.rs")
        example_raw = helpers.read_bounded_regular(root / example, MAX_FILE)
        require(helpers.read_bounded_regular(Path(identities["baseline"]["root"]) / example, MAX_FILE) == example_raw,
                "public profile source differs between arms")
        matrix = cells(opaque, analytic)
        frozen_raw = pinned(binding["inputs"])
        frozen = json.loads(frozen_raw)
        require(type(frozen.get("schema")) is int and frozen["schema"] == 1
                and frozen.get("experiment") == "start-current" and frozen["cells"] == matrix
                and set(frozen["inputs"]) == {input_key(c) for c in matrix}, "frozen input matrix differs")
        for path, pin in frozen["source_files_sha256"].items():
            require(helpers.hash_regular_file(Path(path)) == pin, "fixture-generation source changed")
        retain("inputs-manifest.json", frozen_raw)
        inputs, expected, expected_raw, quality_cache = {}, {}, {}, {}
        for cell in matrix:
            key = input_key(cell)
            if key not in inputs:
                item = frozen["inputs"][key]
                pdf, oracle = pinned(item["pdf"]), pinned(item["oracle"])
                require(paired_render.expected_image(oracle, "full")[:2] == (item["width"], item["height"]),
                        "input dimensions differ")
                inputs[key] = {"pdf": retain(f"inputs/{cell['suite']}/{cell['case']}.pdf", pdf),
                               "oracle": retain(f"oracles/{cell['suite']}/{cell['case']}.ppm", oracle),
                               "width": item["width"], "height": item["height"]}
            if (key, cell["operation"]) not in expected:
                oracle = helpers.read_bounded_regular(directory / inputs[key]["oracle"]["path"], MAX_FILE)
                expected[(key, cell["operation"])] = paired_render.expected_image(oracle, cell["operation"])
                expected_raw[(key, cell["operation"])] = opaque_bytes(expected[(key, cell["operation"])])
        prerequisites = None
        if args.phase == "confirmation":
            prerequisites_raw = pinned({"path": str(args.preflight_pins.resolve()), "sha256": args.preflight_pins_sha256})
            prerequisites = json.loads(prerequisites_raw)
            require(prerequisites.get("schema") == 1 and prerequisites.get("experiment") == "start-current"
                    and prerequisites.get("bindings_sha256") == args.bindings_sha256, "prerequisite binding differs")
            retain("preflight-pins.json", prerequisites_raw)
            for kind in ("preflight", "accuracy"):
                item = prerequisites[kind]
                report_raw, audit_raw = pinned(item["report"], 256 << 20), pinned(item["audit"], 256 << 20)
                prior, audit = json.loads(report_raw), json.loads(audit_raw)
                required = [item["report"]]
                if kind == "preflight":
                    for name in ("plan", "journal"):
                        pinned(item[name], 256 << 20)
                        required.append(item[name])
                    require(prior.get("status") == "completed-preflight" and prior.get("identities") == identities
                            and prior.get("bindings_sha256") == args.bindings_sha256
                            and prior.get("timing_launch_allowed") is True and len(prior["records"]) == 84
                            and all(r.get("C", {}).get("process_success") is True
                                    and r["C"].get("quality", {}).get("correct") is True for r in prior["records"])
                            and audit.get("status") == "verified-preflight"
                            and audit.get("candidate_preflight_correct") is True
                            and audit.get("bindings_sha256") == args.bindings_sha256, "quality preflight does not permit timing")
                else:
                    require(audit.get("status") == "verified-accuracy-probes"
                            and audit.get("candidate_quality_verified") is True, "accuracy probes do not permit timing")
                    for side in SIDES.values():
                        require(all(prior["identities"][side][k] == identities[side][k]
                                    for k in ("root", "commit", "source_sha256")), "accuracy source identity differs")
                require(audit.get("report_sha256") == item["report"]["sha256"], "audit does not bind prerequisite report")
                completed(item, required)
                # Large prior journals/reports remain externally pinned; avoid duplicating them.
        paths = {Path(__file__).resolve(), Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve(),
                 Path("/usr/bin/time"), Path("/bin/ps"), binding_path, root / example}
        for module in tuple(sys.modules.values()):
            path = getattr(module, "__file__", None)
            if path and Path(path).is_file():
                paths.add(Path(path).resolve())
        closure = {str(p): helpers.hash_regular_file(p) for p in sorted(paths)}
        plan = {"schema": 1, "experiment": "start-current", "phase": args.phase, "seed": SEED,
                "warmup_pairs_per_cell": WARMUPS, "measured_pairs_per_cell": PAIRS,
                "expected_attempts": 168 if args.phase == "preflight" else 34440,
                "cells": matrix, "inputs": inputs, "identities": identities, "schedule": schedule(matrix, args.phase),
                "bindings_sha256": args.bindings_sha256, "inputs_manifest_sha256": sha(frozen_raw),
                "bound_files_sha256": closure, "fixture_generation_sources_sha256": frozen["source_files_sha256"],
                "example_sha256": sha(example_raw), "runner_start": runner_start,
                "runner_source_before": start["source_before"], "prerequisites": prerequisites,
                "producer_argv": report["producer_argv"],
                "runtime": {"python": sys.version, "executable": str(Path(sys.executable).resolve()),
                    "launcher": str(Path(sys.orig_argv[0]).resolve()), "platform": platform.platform(),
                    "machine": platform.machine(), "environment": {k: os.environ.get(k) for k in
                        ("LANG", "LC_ALL", "MallocNanoZone", "MallocScribble", "MallocGuardEdges", "MallocStackLogging",
                         "MallocStackLoggingNoCompact", "MALLOC_CONF", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH")}},
                "statistics": {"performed_by": "independent post-completion auditor", "resamples": 5000,
                    "one_sided_confidence": .95, "seed": SEED, "max_rss_ratio": "descriptive; no confidence interval"},
                "scope": "native one-thread default-repair/full-compilation retained API; diagnostics not emitted; page0/72DPI/white; prehashed warm filesystem",
                "limits": {"artifact_bytes": MAX_FILE, "retained_bytes": MAX_RETAINED, "log_bytes": LOG_BUDGET,
                           "child_timeout_seconds": 120},
                "limitations": ["Quality model is frozen analytic RGB/alpha, not arbitrary PDF or continuous-scene perfection.",
                    "Wrong baseline cells retain correction-cost timings and never qualify as equivalent-output speed wins.",
                    "No worldwide, release or aggregate claim; no retry/resume, sample filtering or interim statistics.",
                    "Canceled-child temporary logs may be unavailable; journal and final outer receipt retain cancellation.",
                    "Journal flush does not claim power-loss durability; final outer receipt must bind all terminal artifacts."]}
        plan_record = retain("plan.json", encoded(plan))
        report.update(plan_sha256=plan_record["sha256"], identities=identities, runner_source_before=start["source_before"])
        for sequence, planned in enumerate(plan["schedule"]):
            pair = {**planned, "sequence": sequence, "started_monotonic_ns": time.monotonic_ns()}
            report["records"].append(pair)
            journal.append("pair-start", pair_sequence=sequence, scheduled=planned,
                           started_monotonic_ns=pair["started_monotonic_ns"])
            key, operation, mode = input_key(pair), pair["operation"], pair["mode"]
            expected_image = expected[(key, operation)]
            pdf_path = directory / inputs[key]["pdf"]["path"]
            for label in pair["order"]:
                side, output = SIDES[label], directory / "pending.rgba"
                require(not output.exists(), "unexpected pending output")
                require(helpers.hash_regular_file(binaries[side]) == identities[side]["binary_sha256"]
                        and sha(helpers.read_bounded_regular(pdf_path, MAX_FILE)) == inputs[key]["pdf"]["sha256"],
                        "binary or owned input changed before invocation")
                command = [str(binaries[side]), str(pdf_path), mode, operation, str(output), "72"]
                record = {"command": command, "started_monotonic_ns": time.monotonic_ns(),
                          "process_success": False, "quality": {"correct": False, "reason": "not-completed"}}
                pair[label] = record
                journal.append("arm-start", pair_sequence=sequence, label=label, command=command,
                               started_monotonic_ns=record["started_monotonic_ns"])
                try:
                    process, diagnostic = run_process(command, {"measure_rss": True, "timeout_seconds": 120}, log_budget=LOG_BUDGET)
                    record["process"] = process
                    log_raw = diagnostic.encode("utf-8")
                    record["log"] = retain(f"logs/{sequence:05d}-{label}.stdout-stderr.txt", log_raw)
                    require(diagnostic.encode("ascii", errors="strict") == log_raw and len(log_raw) < LOG_BUDGET,
                            "combined returned log is not complete ASCII")
                    rss = RSS_LINE.findall(diagnostic)
                    require(len(rss) == 1 and re.findall(r"(\d+)\s+maximum resident set size", diagnostic) == rss
                            and type(process.get("peak_rss_bytes")) is int and 0 < int(rss[0]) < 1 << 64
                            and int(rss[0]) == process["peak_rss_bytes"] and process.get("rss_status") == "available",
                            "raw process RSS is missing or inconsistent")
                    record["rss_bytes"] = int(rss[0])
                    record["process_success"] = type(process.get("exit_code")) is int and process["exit_code"] == 0 and process.get("status") != "timeout"
                    if output.exists():
                        raw = helpers.read_bounded_regular(output, MAX_FILE)
                        record["output"] = retain("blobs/" + sha(raw) + ".rgba", raw)
                        cache_key = (record["output"]["sha256"], inputs[key]["oracle"]["sha256"], operation)
                        if cache_key not in quality_cache:
                            quality_cache[cache_key] = ({"correct": True, "expected_width": expected_image[0],
                                "expected_height": expected_image[1], "format_valid": True,
                                "rgb_sha256": sha(expected_image[2]), "rgb_differing_channels": 0,
                                "rgb_maximum_error": 0, "nonopaque_pixels": 0}
                                if raw == expected_raw[(key, operation)] else quality(raw, expected_image))
                        record["quality"] = dict(quality_cache[cache_key])
                        output.unlink()
                    else:
                        record["quality"] = {"correct": False, "reason": "missing-output"}
                    if record["process_success"]:
                        stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
                        require(len(stages) == 1 and isinstance(stages[0], dict), "unique stage record missing")
                        stage = stages[0]
                        require(all(type(stage.get(k)) is int and stage[k] > 0 for k in
                                    ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
                                and type(stage.get("schema")) is int and stage["schema"] == 1
                                and stage.get("accuracy") == mode and stage.get("operation") == operation
                                and type(stage.get("dpi")) is int and stage["dpi"] == 72
                                and all(type(stage.get(k)) is int for k in ("width", "height"))
                                and (stage.get("width"), stage.get("height")) == expected_image[:2]
                                and stage["metric_ns"] == stage["total_ns" if operation == "cold" else "render_ns"]
                                and stage["total_ns"] >= stage["open_ns"] + stage["compile_ns"] + stage["render_ns"],
                                "stage identity or timing relationship differs")
                        record["stages"] = stage
                except BaseException as error:
                    record["error"] = f"{type(error).__name__}: {error}"
                    if isinstance(error, (KeyboardInterrupt, SystemExit)):
                        record["interrupted"] = True
                    if output.exists() and "output" not in record:
                        try:
                            raw = helpers.read_bounded_regular(output, MAX_FILE)
                            record["output"] = retain("blobs/" + sha(raw) + ".rgba", raw)
                        except (OSError, ValueError) as artifact_error:
                            record["artifact_error"] = str(artifact_error)
                    raise
                finally:
                    record["finished_monotonic_ns"] = time.monotonic_ns()
                    journal.append("arm-result", pair_sequence=sequence, label=label, record=record)
            pair["classification"] = classification(pair)
            pair["raw_equal"] = (pair["B"].get("output", {}).get("sha256") == pair["C"].get("output", {}).get("sha256")
                                 if "output" in pair["B"] and "output" in pair["C"] else None)
            pair["finished_monotonic_ns"] = time.monotonic_ns()
            journal.append("pair-finished", pair_sequence=sequence, classification=pair["classification"],
                           raw_equal=pair["raw_equal"], finished_monotonic_ns=pair["finished_monotonic_ns"])
            if (sequence + 1) % len(matrix) == 0:
                print(f"completed {args.phase} round {(sequence + 1) // len(matrix)}; all84 cells retained", flush=True)
        for side, identity in identities.items():
            require(helpers.hash_regular_file(binaries[side]) == identity["binary_sha256"]
                    and paired_render.source_identity(identity["root"]) ==
                    {k: identity[k] for k in ("root", "commit", "source_sha256")}, "source or binary changed")
        require(all(helpers.hash_regular_file(Path(path)) == pin for path, pin in closure.items()), "loaded source/runtime changed")
        require(all(helpers.hash_regular_file(Path(path)) == pin for path, pin in frozen["source_files_sha256"].items()),
                "fixture-generation source changed during collection")
        require(len(report["records"]) * 2 == plan["expected_attempts"], "fixed attempt count differs")
        candidate_correct = all(r["C"]["process_success"] and r["C"]["quality"]["correct"] for r in report["records"])
        report.update(status="completed-" + args.phase, candidate_quality_correct=candidate_correct,
                      timing_launch_allowed=candidate_correct if args.phase == "preflight" else False,
                      attempted_processes=plan["expected_attempts"])
        return_code = 0 if candidate_correct else 1
    except BaseException as error:
        report.update(status="cancelled" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "failed",
                      error=f"{type(error).__name__}: {error}", timing_launch_allowed=False)
        return_code = 128 + getattr(error, "signum", signal.SIGINT) if report["status"] == "cancelled" else 1
    finally:
        try:
            report.update(finished_unix_ns=time.time_ns(), duration_ns=time.monotonic_ns() - started)
            try:
                journal.append("run-finished", status=report["status"], finished_unix_ns=report["finished_unix_ns"],
                               duration_ns=report["duration_ns"], error=report.get("error"))
            except (OSError, ValueError) as error:
                report.update(status="failed", journal_error=str(error))
                return_code = 1
            finally:
                journal.close()
            report["journal"] = {"path": "events.jsonl", "bytes": journal.bytes, "events": journal.events,
                                 "sha256": helpers.hash_regular_file(directory / "events.jsonl") if helpers else None}
            save_report()
        finally:
            journal.close()
            for signum, handler in previous.items():
                signal.signal(signum, handler)
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
