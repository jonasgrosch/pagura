#!/usr/bin/env python3
"""Reuse the full raw-output audit, then report RSS bounds and retention prerequisites."""
import json
from pathlib import Path
import sys

import rectangle_reserve as experiment
from evaluate_n16_output import ROOT, audit, load, require
from benchkit.artifacts import hash_regular_file
from benchkit.statistics import ratio_bounds

HARNESSES = {
    "benchmarks/rectangle_reserve.py", "benchmarks/color_reuse_fixtures.py",
    "benchmarks/paired_render.py",
    "benchmarks/results/sources/analytic19-conversion/manifest.json",
    "benchmarks/results/sources/analytic19-conversion/references.json",
}


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_rectangle_reserve.py pilot-001|confirmation-001")
    run = sys.argv[1]
    results = ROOT / "benchmarks/results"
    local = results / "local/rectangle-reserve"
    plan = load(results / "rectangle-reserve-plan.json")
    bindings = load(results / "rectangle-reserve-bindings.json")
    require(plan["status"] == "frozen-before-pilot", "plan not frozen")
    require(plan["baseline_commit"] == bindings["identities"]["baseline"]["commit"]
            == experiment.BASELINE_COMMIT, "wrong frozen baseline")
    require(plan["target_priority"] == experiment.TARGET_PRIORITY, "target priority drift")
    require(plan["partial_retention"] == {
        "primary_median_upper_strictly_below": 1.0,
        "any_latency_lower_material_regression_above": 1.02,
        "observed_peak_rss_investigation_above": 1.05,
        "automatic_retention": False,
    }, "partial-retention criteria drift")
    experiment.require_baseline(bindings["identities"]["baseline"]["root"])
    # Validate the exact direct launch chain, not just a successful observer.
    directory = local / run
    command = ["python3", "-B", "benchmarks/rectangle_reserve.py"]
    for side in ("baseline", "candidate"):
        command.extend(["--" + side, bindings["identities"][side]["binary"]])
    for side in ("baseline", "candidate"):
        command.extend(["--" + side + "-source", bindings["identities"][side]["root"]])
    command.extend(["--output", str(directory), "--phase", run.split("-", 1)[0]])
    guard_path = local / (run + "-observer/guard.json")
    guard, receipt = load(guard_path), load(local / (run + "-receipt.json"))
    require(guard["command"] == command, "unexpected benchmark child command")
    observer = "benchmarks/results/sources/analytic19-conversion/load_observer.py"
    require(receipt["argv"] == ["python3", "-B", observer, "--evidence-dir",
                                str(guard_path.parent), "--", *command], "unexpected observer invocation")
    require(bindings["bound_files_sha256"].get(observer) == hash_regular_file(ROOT / observer),
            "strict observer source is not pinned")
    audit(experiment_module=experiment, experiment_name="rectangle-reserve", stem="rectangle-reserve",
          local_name="rectangle-reserve", run=run, expected_case_count=19, expected_cell_count=38,
          target_case=experiment.TARGET_CASE, target_cases=experiment.TARGET_CASES,
          target_operation=experiment.TARGET_OPERATION, expected_guard_mode="strict",
          expected_harness_sources=HARNESSES, audit_entrypoint="benchmarks/evaluate_rectangle_reserve.py")
    # Audit above authenticates final receipt/report/guard, fixed schedule,
    # all warmups and samples, raw RGBA and independent frozen RGB oracles.
    report = load(directory / "report.json")
    analysis_path = local / (run + "-analysis.json")
    analysis = load(analysis_path)
    require(hash_regular_file(directory / "report.json") == analysis["report_sha256"],
            "report changed after raw-output audit")
    raw_cells = {(cell["case"], cell["mode"], cell["operation"]): cell for cell in report["cells"]}
    for cell in analysis["cells"]:
        key = (cell["case"], cell["mode"], cell["operation"])
        pairs = [pair for pair in raw_cells[key]["pairs"] if pair["pair"] >= 0]
        cell["rss_bounds"] = ratio_bounds(
            [pair["candidate"]["process"]["peak_rss_bytes"] for pair in pairs],
            [pair["baseline"]["process"]["peak_rss_bytes"] for pair in pairs])
        cell["priority"] = experiment.TARGET_PRIORITY.get(cell["case"], "protected")
    rows = analysis["cells"]
    primary = [cell for cell in rows if cell["priority"] == "primary"]
    secondary = [cell for cell in rows if cell["priority"] == "secondary"]
    require(len(primary) == len(secondary) == 2, "missing primary or secondary modes")
    regressions = [{key: cell[key] for key in ("case", "mode", "operation", "latency")}
                   for cell in rows if any(cell["latency"][name + "_lower"] > 1.02 for name in ("median", "p95"))]
    rss_investigation = [{key: cell[key] for key in ("case", "mode", "operation", "peak_rss_ratio", "rss_bounds")}
                         for cell in rows if cell["peak_rss_ratio"] > 1.05]
    primary_benefit = any(cell["latency"]["median_upper"] < 1.0 for cell in primary)
    analysis.update(
        audit_entrypoint_sha256=hash_regular_file(Path(__file__)),
        shared_evaluator_sha256=hash_regular_file(ROOT / "benchmarks/evaluate_n16_output.py"),
        rss_bounds_recomputed_from_raw_pairs=True,
        rss_bounds_scope="Marginal paired-bootstrap median/p95 of process peak RSS; not a confidence bound on maximum or future memory; existing observed maximum ratio gate remains unchanged",
        primary_latency_passes=sum(cell["median_pass"] and cell["p95_pass"] for cell in primary),
        secondary_latency_passes=sum(cell["median_pass"] and cell["p95_pass"] for cell in secondary),
        primary_median_benefit=primary_benefit,
        supported_material_latency_regressions=regressions,
        rss_investigation_required=rss_investigation,
        partial_numeric_prerequisites_met=run == "confirmation-001" and primary_benefit and not regressions and not rss_investigation,
        retention_decision="not made; coordinator must review validation, resource/error semantics, all misses and any RSS investigation",
        admission=False,
    )
    analysis_path.write_text(json.dumps(analysis, indent=2) + "\n")
    print(json.dumps({name: value for name, value in analysis.items() if name != "cells"}, indent=2))


if __name__ == "__main__":
    main()
