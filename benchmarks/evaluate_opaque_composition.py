#!/usr/bin/env python3
"""Audit the fixed shared-opaque experiment; never grant release admission.

Rehash bound sources, receipts and every distinct output; regenerate independent
oracles and verify every sample's identity. Point estimates are recomputed from
raw timings. Bootstrap bounds are reported estimates from the pinned harness,
not independently rerun here. Source-to-binary provenance still trusts the
coordinator's build record, and observation-only monitoring is not exclusivity.
"""
import hashlib
import json
import math
from pathlib import Path
import random
import statistics
import sys

import opaque_composition as experiment
from paired_render import ROOT, MAX_BYTES, expected_image
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.statistics import quantile

EXPERIMENT = "shared-opaque-composition"
THRESHOLDS = {"target_median_upper": .95, "target_p95_upper": 1.0,
              "protected_median_upper": 1.02, "protected_p95_upper": 1.02,
              "peak_rss_ratio_max": 1.0}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return read_bounded_regular(path, MAX_BYTES)


def load(path):
    return json.loads(read(path))


def positive_int(value):
    return type(value) is int and value > 0


def key(cell):
    return cell["case"], cell["mode"], cell["operation"]


def cell_map(cells):
    result = {key(cell): cell["role"] for cell in cells}
    require(len(result) == len(cells) == 46, "expected 46 unique cells")
    return result


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_opaque_composition.py pilot-001|confirmation-001")
    run = sys.argv[1]
    phase_name = run.split("-", 1)[0]
    results = ROOT / "benchmarks/results"
    local = results / "local/opaque-correction"
    directory = local / run
    report_path = directory / "report.json"
    receipt_path = local / (run + "-receipt.json")
    guard_path = local / (run + "-observer/guard.json")
    bindings_path = results / "opaque-composition-bindings.json"
    plan_path = results / "opaque-composition-plan.json"
    report, receipt, guard = load(report_path), load(receipt_path), load(guard_path)
    bindings, plan = load(bindings_path), load(plan_path)
    require(report["experiment"] == plan["experiment"] == EXPERIMENT, "wrong experiment")
    require(report["phase"] == phase_name, "run name and phase differ")
    require(report["status"] == "complete-exploratory", "incomplete report")
    require(receipt["status"] == "completed" and receipt["source_unchanged"], "invalid runner receipt")
    require(receipt["exit_code"] == receipt["wrapper_exit_code"] == 0, "runner failed")
    require(not receipt["errors"] and not receipt["artifact_errors"], "runner evidence errors")
    require(receipt["source_before"] == receipt["source_after"], "runner source changed")
    require(receipt["source_before"]["sha256"] == bindings["candidate_runner_source_sha256"],
            "runner source differs from frozen binding")
    require(Path(receipt["source_root"]).resolve() == ROOT.resolve(), "wrong runner checkout")
    require(report["environment"]["run_id"] == receipt["run_id"], "run ID mismatch")
    require(Path(report["environment"]["run_receipt"]).resolve() == receipt_path.resolve(),
            "wrong report receipt pointer")
    require(guard["status"] == "completed-observation-only" and guard["exit_code"] == 0,
            "observer did not complete")
    for path in (report_path, guard_path):
        require(receipt["artifacts"][str(path)] == hash_regular_file(path), "receipt artifact mismatch")
    require(report["identities"] == bindings["identities"], "engine identity mismatch")
    for side, identity in report["identities"].items():
        require(side in ("baseline", "candidate"), "unexpected engine side")
        require(hash_regular_file(Path(identity["binary"])) == identity["binary_sha256"],
                side + " binary changed")
    bound = bindings["bound_files_sha256"]
    required_files = {"benchmarks/opaque_composition.py", "benchmarks/evaluate_opaque_composition.py",
                      "benchmarks/paired_render.py", "benchmarks/glyph_memo.py",
                      "benchmarks/glyph_memo_fixtures.py", "benchmarks/scene_index.py",
                      "benchmarks/clip_viewport_fixtures.py",
                      "crates/pagura-render/examples/scene_index_profile.rs",
                      "benchmarks/results/opaque-composition-plan.json"}
    require(required_files <= set(bound), "missing harness or plan binding")
    for name, expected in bound.items():
        path = (ROOT / name).resolve()
        require(path.is_relative_to(ROOT.resolve()), "bound file escapes checkout")
        require(hash_regular_file(path) == expected, "bound file changed: " + name)
    harness_names = required_files - {"benchmarks/evaluate_opaque_composition.py",
                                     "crates/pagura-render/examples/scene_index_profile.rs",
                                     "benchmarks/results/opaque-composition-plan.json"}
    require(set(report["harness_sources_sha256"]) == harness_names, "incomplete harness source set")
    for name, expected in report["harness_sources_sha256"].items():
        require(bound[name] == expected, "report harness binding mismatch")
    require(report["driver_sha256"] == bound["benchmarks/opaque_composition.py"], "wrong driver")
    require(report["example_sha256"] == bound["crates/pagura-render/examples/scene_index_profile.rs"],
            "wrong public API example")
    require(report["thresholds"] == plan["thresholds"] == THRESHOLDS, "threshold drift")
    require(plan["analysis"] == {"method": "paired-percentile-bootstrap", "resamples": 5000,
                                 "confidence": .95, "seed": 42017}, "analysis plan drift")
    require({name: report["statistics"][name] for name in
             ("method", "resamples", "one_sided_confidence", "seed")}
            == {"method": "paired percentile bootstrap", "resamples": 5000,
                "one_sided_confidence": .95, "seed": 42017}, "reported analysis settings drift")
    require(report["phase_plan"] == plan["phases"] == experiment.PHASE_PLAN, "phase plan drift")
    phase = experiment.PHASE_PLAN[phase_name]
    require({name: report[name] for name in phase} == phase, "sample count or seed drift")
    require(report["dpi"] == 72, "wrong DPI")
    require(len(experiment.CASES) == len(set(experiment.CASES)) == 16, "expected 16 unique cases")
    declared = {cell: "target" if cell[0] == "opaque64" and cell[2] == "full" else "protected"
                for cell in experiment.CELLS}
    require(len(declared) == len(experiment.CELLS) == 46, "entrypoint cell drift")
    for cells in (plan["cells"], report["declared_cells"], report["cells"]):
        require(cell_map(cells) == declared, "cell identity or target/protected role drift")
    require(set(report["inputs"]) == set(experiment.CASES), "input case drift")
    for name in experiment.CASES:
        pdf, oracle = experiment.fixture(name)
        for kind, data, extension in (("inputs", pdf, "pdf"), ("oracles", oracle, "ppm")):
            require(read(directory / kind / (name + "." + extension)) == data, "fixture differs: " + name)
        require(hashlib.sha256(pdf).hexdigest() == report["inputs"][name]["pdf_sha256"], "input hash mismatch")
        require(hashlib.sha256(oracle).hexdigest() == report["inputs"][name]["oracle_sha256"], "oracle hash mismatch")
    rng = random.Random(phase["seed"])
    ordered = list(experiment.CELLS)
    rng.shuffle(ordered)
    require([key(cell) for cell in report["cells"]] == ordered, "randomized cell order mismatch")
    rows, blobs, outputs = [], set(), 0
    for cell in report["cells"]:
        pairs = cell["pairs"]
        require([p["pair"] for p in pairs] == list(range(-phase["warmups"], phase["pairs"])),
                "missing, reordered, or duplicate warmup/measured pairs")
        expected = expected_image(read(directory / "oracles" / (cell["case"] + ".ppm")), cell["operation"])
        expected_pixels_sha256 = hashlib.sha256(expected[2]).hexdigest()
        checked = set()
        for pair in pairs:
            order = ["baseline", "candidate"]
            rng.shuffle(order)
            require(pair["order"] == order, "paired order disagrees with frozen seed")
            require(pair["baseline"]["output_sha256"] == pair["candidate"]["output_sha256"], "paired output differs")
            for side in ("baseline", "candidate"):
                record = pair[side]
                # run_process leaves provisional status='failed' even at exit
                # zero; this harness establishes success by checking raw output.
                require(record["oracle_equal"] and record["process"]["exit_code"] == 0
                        and record["process"]["status"] in ("failed", "ok"), "failed sample")
                require(positive_int(record["process"]["peak_rss_bytes"]), "missing or invalid sample RSS")
                command = [report["identities"][side]["binary"], str(directory / "inputs" / (cell["case"] + ".pdf")),
                           cell["mode"], cell["operation"], str(directory / "pending.rgba"), "72"]
                require(record["command"] == command, "sample command mismatch")
                stages = record["stages"]
                require(all(positive_int(stages.get(name)) for name in
                            ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")), "invalid stage timing")
                require(stages["schema"] == 1 and stages["operation"] == cell["operation"]
                        and stages["accuracy"] == cell["mode"] and stages["dpi"] == 72
                        and (stages["width"], stages["height"]) == expected[:2], "stage identity mismatch")
                require(stages["metric_ns"] == stages["total_ns" if cell["operation"] == "cold" else "render_ns"]
                        and stages["total_ns"] >= sum(stages[n] for n in ("open_ns", "compile_ns", "render_ns")),
                        "wrong timing metric")
                require((record["width"], record["height"]) == expected[:2]
                        and record["pixels_sha256"] == expected_pixels_sha256, "pixel metadata mismatch")
                blob = record["blob"]
                require(len(record["output_sha256"]) == 64
                        and all(c in "0123456789abcdef" for c in record["output_sha256"]), "invalid output digest")
                require(blob == "blobs/" + record["output_sha256"] + ".rgba", "invalid blob identity")
                if blob not in checked:
                    raw = read(directory / blob)
                    require(hashlib.sha256(raw).hexdigest() == record["output_sha256"], "raw blob hash mismatch")
                    require(normalize(raw) == expected, "independent oracle mismatch")
                    header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
                    require(raw.startswith(header) and raw[len(header) + 3::4] == b"\xff" * (expected[0] * expected[1]),
                            "nonopaque alpha or unexpected raw format")
                    checked.add(blob)
                    blobs.add(blob)
                outputs += 1
        measured = [p for p in pairs if p["pair"] >= 0]
        values = {side: [p[side]["stages"]["metric_ns"] for p in measured] for side in ("baseline", "candidate")}
        latency = cell["latency"]
        require(set(latency) == {name + suffix for name in ("median", "p95") for suffix in ("_ratio", "_lower", "_upper")},
                "missing reported bootstrap statistics")
        require(all(type(v) in (int, float) and math.isfinite(v) and v > 0 for v in latency.values()), "invalid latency statistic")
        for name, statistic in (("median", statistics.median), ("p95", lambda v: quantile(v, .95))):
            require(latency[name + "_ratio"] == statistic(values["candidate"]) / statistic(values["baseline"]), "point ratio mismatch")
            require(latency[name + "_lower"] <= latency[name + "_upper"], "reversed bootstrap bounds")
        peaks = {side: max(p[side]["process"]["peak_rss_bytes"] for p in measured) for side in values}
        rss_ratio = peaks["candidate"] / peaks["baseline"]
        require(rss_ratio == cell["peak_rss_ratio"], "RSS ratio mismatch")
        prefix = cell["role"]
        row = {name: cell[name] for name in ("case", "mode", "operation", "role")}
        row.update(latency=latency, peak_rss_ratio=rss_ratio,
                   peak_rss_mib={side: value / 2**20 for side, value in peaks.items()},
                   median_pass=latency["median_upper"] <= THRESHOLDS[prefix + "_median_upper"],
                   p95_pass=latency["p95_upper"] <= THRESHOLDS[prefix + "_p95_upper"],
                   memory_pass=rss_ratio <= THRESHOLDS["peak_rss_ratio_max"])
        for side in values:
            row[side + "_median_ms"] = statistics.median(values[side]) / 1e6
        rows.append(row)
    contention = any(event["matches"] for event in guard["events"])
    summary = {"schema": 1, "experiment": EXPERIMENT, "run": run, "phase": phase_name, "admission": False,
               "background_contention_observed": contention, "observed_contention_blocks_admission": contention,
               "report_sha256": hash_regular_file(report_path), "receipt_sha256": hash_regular_file(receipt_path),
               "guard_sha256": hash_regular_file(guard_path), "plan_sha256": hash_regular_file(plan_path),
               "bindings_sha256": hash_regular_file(bindings_path), "independently_verified_outputs": outputs,
               "distinct_output_blobs_rehashed": len(blobs), "bootstrap_bounds_independently_recomputed": False,
               "numeric_gates_pass": all(c["median_pass"] and c["p95_pass"] and c["memory_pass"] for c in rows),
               "target_latency_passes": sum(c["median_pass"] and c["p95_pass"] for c in rows if c["role"] == "target"),
               "protected_latency_passes": sum(c["median_pass"] and c["p95_pass"] for c in rows if c["role"] == "protected"),
               "memory_passes": sum(c["memory_pass"] for c in rows), "cells": rows}
    (local / (run + "-analysis.json")).write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({name: value for name, value in summary.items() if name != "cells"}, indent=2))
    for cell in rows:
        if cell["role"] == "target" or cell["case"].startswith("opaque-"):
            print(cell["case"], cell["mode"], cell["operation"], "median ratio", round(cell["latency"]["median_ratio"], 4),
                  "p95 upper", round(cell["latency"]["p95_upper"], 4), "RSS ratio", round(cell["peak_rss_ratio"], 4))


if __name__ == "__main__":
    main()
