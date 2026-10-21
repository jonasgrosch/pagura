#!/usr/bin/env python3
"""Fixed accuracy-cost diagnostic; mismatching baseline pixels are never equivalent work."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import statistics
import sys

ROOT = Path.cwd().resolve()
sys.path.insert(0, str(ROOT / "benchmarks"))
from paired_render import MAX_BYTES, MAX_RETAINED, source_identity
import color_reuse_fixtures as fixtures
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.measurement import run_process

CELLS = tuple((case, mode, "cold") for case in fixtures.CASES for mode in ("exact", "competitive"))
SEED, WARMUPS, PAIRS = 20261110, 5, 31
PLAN = {"schema": 1, "cases": list(fixtures.CASES), "cells": [list(cell) for cell in CELLS],
        "metric": "total_ns", "seed": SEED, "warmup_pairs": WARMUPS, "measured_pairs": PAIRS,
        "expected_preflight_outputs": 76, "expected_cost_outputs": 2736,
        "measured_order_per_cell": {"baseline_first": 16, "candidate_first": 15},
        "warmup_order_per_cell": {"baseline_first": 3, "candidate_first": 2}}
sha = lambda raw: hashlib.sha256(raw).hexdigest()
read = lambda path: read_bounded_regular(Path(path), MAX_BYTES)


def quality(raw, expected):
    actual = normalize(raw)
    header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
    dimensions = actual[:2] == expected[:2]
    opaque = raw.startswith(header) and raw[len(header) + 3::4] == b"\xff" * (expected[0] * expected[1])
    errors = [abs(a - b) for a, b in zip(actual[2], expected[2])] if dimensions else []
    return {"dimensions_equal": dimensions, "opaque_alpha_equal": opaque, "rgb_equal": actual == expected,
            "max_channel_error": max(errors, default=0) if dimensions else None,
            "different_channels": sum(value != 0 for value in errors) if dimensions else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bindings", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--phase", required=True, choices=("preflight", "cost"))
    parser.add_argument("--preflight", type=Path)
    parser.add_argument("--preflight-sha256")
    args = parser.parse_args()
    if not os.environ.get("PAGURA_RUN_ID") or not os.environ.get("PAGURA_RUN_RECEIPT"):
        parser.error("the serial coordinator's run ID and receipt are required")
    if (args.phase == "cost" and not (args.preflight and args.preflight_sha256)) or (args.phase == "preflight" and (args.preflight or args.preflight_sha256)):
        parser.error("cost requires a pinned preflight; preflight accepts neither preflight argument")
    bindings_raw = read(args.bindings)
    bindings = json.loads(bindings_raw)
    identities = bindings["identities"]
    if bindings["schema"] != 1 or bindings["plan"] != PLAN or set(identities) != {"baseline", "candidate"} or Path(identities["candidate"]["root"]).resolve() != ROOT:
        raise ValueError("wrong binding schema or source roots")
    files = (Path(__file__).resolve(), ROOT / "benchmarks/paired_render.py", Path(fixtures.__file__).resolve(), fixtures.MANIFEST, fixtures.REFERENCES)
    harness = {str(path): hash_regular_file(path) for path in files}
    if harness != bindings["harness_sha256"] or not bindings["evidence"]:
        raise ValueError("missing frozen harness or completed build evidence")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    rows, inputs, expected, anchors, artifacts = [], {}, {}, {}, {}
    stored = 0
    report = {"schema": 1, "plan": PLAN, "experiment": "f32-preservation-cost", "phase": args.phase, "status": "incomplete",
              "admission": False, "identities": identities, "harness_sha256": harness, "bindings_sha256": sha(bindings_raw),
              "run_id": os.environ["PAGURA_RUN_ID"], "receipt": os.environ["PAGURA_RUN_RECEIPT"], "artifacts": artifacts,
              "limitations": ["Descriptive accuracy cost, not equivalent-output speed superiority or release admission",
                              "Fixed analytic19 only; no shading performance or large dynamic-scene coverage",
                              "Native public compile_page+render example; no browser/WASM or competitor claim",
                              "Live receipt copies are snapshots; final outer receipt and observer must be pinned externally"]}

    def retain(raw):
        nonlocal stored
        relative = "blobs/" + sha(raw)
        path = output / relative
        if relative not in artifacts:
            if stored + len(raw) > MAX_RETAINED:
                raise RuntimeError("retention budget exhausted")
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(raw)
            artifacts[relative] = {"sha256": sha(raw), "bytes": len(raw)}
            stored += len(raw)
        elif read(path) != raw:
            raise RuntimeError("retained artifact changed")
        return relative

    def check_sources():
        for identity in identities.values():
            if source_identity(identity["root"]) != {key: identity[key] for key in ("root", "commit", "source_sha256")}:
                raise RuntimeError("frozen source changed")
            if hash_regular_file(Path(identity["binary"])) != identity["binary_sha256"]:
                raise RuntimeError("frozen binary changed")
        if {str(path): hash_regular_file(path) for path in files} != harness:
            raise RuntimeError("frozen harness changed")

    def sample(cell, side, warmup, pair):
        case, mode, operation = cell
        binary = identities[side]["binary"]
        if hash_regular_file(Path(binary)) != identities[side]["binary_sha256"] or read(output / inputs[case]["pdf"]) != inputs[case]["bytes"]:
            raise RuntimeError("binary or retained input changed")
        pending = output / "pending.rgba"
        pending.unlink(missing_ok=True)
        command = [binary, str(output / inputs[case]["pdf"]), mode, operation, str(pending), "72"]
        row = {"cell": list(cell), "side": side, "warmup": warmup, "pair": pair, "command": command, "ready": False}
        rows.append(row)
        process, diagnostic = run_process(command, {"timeout_seconds": 120, "maximum_output_bytes": MAX_BYTES, "measure_rss": True})
        row.update(process=process, diagnostic=retain(diagnostic.encode()))
        try:
            raw = read(pending)
        except (OSError, ValueError) as error:
            row["error"] = f"unreadable output: {error}"
            raw = None
        if raw is not None:
            row.update(output=retain(raw), output_sha256=sha(raw))
        try:
            if raw is None:
                raise ValueError(row["error"])
            row["quality"] = quality(raw, expected[case])
            records = []
            for line in diagnostic.splitlines():
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict) and value.get("schema") == 1:
                    records.append(value)
            if len(records) != 1:
                raise ValueError("missing or ambiguous stage record")
            stages = records[0]
            row["stages"] = stages
            if any(type(stages.get(key)) is not int or stages[key] <= 0 for key in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")):
                raise ValueError("invalid stage measurements")
            if (stages.get("operation"), stages.get("accuracy"), stages.get("dpi"), stages.get("width"), stages.get("height")) != (operation, mode, 72, *expected[case][:2]) or stages["metric_ns"] != stages["total_ns"] or stages["total_ns"] < sum(stages[key] for key in ("open_ns", "compile_ns", "render_ns")):
                raise ValueError("stage settings or total mismatch")
            if any(type(process.get(key)) is not int or process[key] <= 0 for key in ("elapsed_ns", "peak_rss_bytes")):
                raise ValueError("missing elapsed time or per-process RSS")
            q = row["quality"]
            row["ready"] = process.get("exit_code") == 0 and process.get("status") != "timeout" and q["dimensions_equal"] and q["opaque_alpha_equal"] and (side == "baseline" or q["rgb_equal"])
            if args.phase == "cost" and side == "baseline":
                row["baseline_preflight_equal"] = sha(raw) == anchors[cell]["output_sha256"]
                row["ready"] &= row["baseline_preflight_equal"]
        except ValueError as error:
            row["error"] = f"{type(error).__name__}: {error}"
        with (output / "records.jsonl").open("a") as stream:
            stream.write(json.dumps(row) + "\n")

    try:
        check_sources()
        report["bindings"] = retain(bindings_raw)
        report["evidence"] = []
        for evidence in bindings["evidence"]:
            raw = read(evidence["path"])
            if sha(raw) != evidence["sha256"]:
                raise ValueError("completed coordinator evidence changed")
            report["evidence"].append({**evidence, "artifact": retain(raw)})
        report["receipt_start_snapshot"] = retain(read(report["receipt"]))
        for case in fixtures.CASES:
            pdf, oracle = fixtures.fixture(case)
            inputs[case] = {"pdf": retain(pdf), "oracle": retain(oracle), "bytes": pdf}
            expected[case] = normalize(oracle)
        report["inputs"] = {case: {key: value for key, value in item.items() if key != "bytes"} for case, item in inputs.items()}
        if args.phase == "preflight":
            for cell in CELLS:
                for side in ("baseline", "candidate"):
                    sample(cell, side, False, 0)
            report["samples"] = rows
            report["baseline_rgb_mismatches"] = [{"cell": row["cell"], "quality": row["quality"]} for row in rows if row["side"] == "baseline" and "quality" in row and not row["quality"]["rgb_equal"]]
        else:
            raw = read(args.preflight)
            if sha(raw) != args.preflight_sha256:
                raise ValueError("preflight report changed")
            prior = json.loads(raw)
            report["preflight"] = retain(raw)
            if any(prior.get(key) != report[key] for key in ("schema", "experiment", "plan", "identities", "harness_sha256", "inputs")) or prior.get("phase") != "preflight" or prior.get("status") != "quality-ready" or len(prior["samples"]) != 76:
                raise ValueError("preflight is incomplete or belongs to another experiment")
            for (cell, side), row in zip(((cell, side) for cell in CELLS for side in ("baseline", "candidate")), prior["samples"]):
                raw = read(args.preflight.parent / row["output"])
                q = quality(raw, expected[cell[0]])
                if row["cell"] != list(cell) or row["side"] != side or sha(raw) != row["output_sha256"] or not row["ready"] or not q["dimensions_equal"] or not q["opaque_alpha_equal"] or (side == "candidate" and not q["rgb_equal"]):
                    raise ValueError("preflight output binding or quality mismatch")
                if side == "baseline":
                    anchors[cell] = {"output_sha256": sha(raw), "rgb_equal": q["rgb_equal"]}
            rng, schedule = random.Random(SEED), []
            for warmup, count in ((True, WARMUPS), (False, PAIRS)):
                phase = []
                for cell in CELLS:
                    orders = [("baseline", "candidate")] * ((count + 1) // 2) + [("candidate", "baseline")] * (count // 2)
                    rng.shuffle(orders)
                    phase.extend((cell, warmup, index, order) for index, order in enumerate(orders))
                rng.shuffle(phase)
                schedule.extend(phase)
            report["schedule"] = retain(json.dumps(schedule).encode())
            report.update(seed=SEED, warmup_pairs=WARMUPS, measured_pairs=PAIRS)
            for cell, warmup, pair, order in schedule:
                for side in order:
                    sample(cell, side, warmup, pair)
            report["summary"] = []
            for cell in CELLS:
                selected = [row for row in rows if row["cell"] == list(cell)]
                summary = {"cell": list(cell), "baseline_matches_oracle": anchors[cell]["rgb_equal"], "complete_and_valid": len(selected) == 72 and all(row["ready"] for row in selected)}
                if summary["complete_and_valid"]:
                    summary["medians"] = {side: {metric: statistics.median((row["stages"] if metric in ("total_ns", "compile_ns", "render_ns") else row["process"])[metric] for row in selected if row["side"] == side and not row["warmup"]) for metric in ("total_ns", "compile_ns", "render_ns", "elapsed_ns", "peak_rss_bytes")} for side in ("baseline", "candidate")}
                    summary["measured_maximum_peak_rss_bytes"] = {side: max(row["process"]["peak_rss_bytes"] for row in selected if row["side"] == side and not row["warmup"]) for side in ("baseline", "candidate")}
                report["summary"].append(summary)
        check_sources()
        if len(rows) != (76 if args.phase == "preflight" else 2736):
            raise RuntimeError("fixed matrix incomplete")
        report["status"] = ("quality-ready" if args.phase == "preflight" else "completed-diagnostic") if all(row["ready"] for row in rows) else "blocked-quality-or-process"
    except BaseException as error:
        report.update(status="invalid-or-incomplete", fatal_error=f"{type(error).__name__}: {error}", last_record=rows[-1] if rows else None)
        raise
    finally:
        report.update(completed_records=len(rows), failure_records=[index for index, row in enumerate(rows) if not row["ready"]])
        if (output / "records.jsonl").exists():
            report["records_sha256"] = hash_regular_file(output / "records.jsonl")
        try:
            report["receipt_end_snapshot"] = retain(read(report["receipt"]))
        except (OSError, ValueError, RuntimeError) as error:
            report.update(status="invalid-or-incomplete", receipt_error=f"{type(error).__name__}: {error}")
        report["stored_blob_bytes"] = stored
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "report": str(output / "report.json")}))
    return 0 if report["status"] in ("quality-ready", "completed-diagnostic") else 1


if __name__ == "__main__":
    raise SystemExit(main())
