#!/usr/bin/env python3
"""Audit 52 fixed painted-interval cells; retention remains a coordinator decision."""
import sys

import n16_painted_interval as experiment
from evaluate_n16_output import ROOT, audit, load, require


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_n16_painted_interval.py pilot-001|confirmation-001")
    results = ROOT / "benchmarks/results"
    plan = load(results / "n16-painted-interval-plan.json")
    bindings = load(results / "n16-painted-interval-bindings.json")
    require(plan["status"] == "frozen-before-pilot", "painted-interval plan is not frozen")
    expected = experiment.baseline_commit()
    baseline = bindings["identities"]["baseline"]
    require(baseline["commit"] == expected, "baseline identity differs from integrated commit")
    for side, identity in bindings["identities"].items():
        require(all(identity[key] == plan["identities_to_freeze"][side][key]
                    for key in ("root", "commit", "source_sha256", "binary", "binary_sha256")),
                "native binding differs from frozen plan: " + side)
    experiment.require_baseline(baseline["root"])
    audit(experiment_module=experiment, experiment_name="n16-painted-interval",
          stem="n16-painted-interval", local_name="n16-painted-interval", run=sys.argv[1],
          expected_case_count=18, expected_cell_count=52, expected_guard_mode="strict",
          expected_harness_sources={
              "benchmarks/n16_painted_interval.py", "benchmarks/n16_painted_interval_fixtures.py",
              "benchmarks/n16_output_u16.py", "benchmarks/n16_output_guarded.py",
              "benchmarks/n16_output.py", "benchmarks/opaque_composition.py",
              "benchmarks/paired_render.py", "benchmarks/glyph_memo.py",
              "benchmarks/glyph_memo_fixtures.py", "benchmarks/scene_index.py",
              "benchmarks/clip_viewport_fixtures.py",
          }, audit_entrypoint="benchmarks/evaluate_n16_painted_interval.py")


if __name__ == "__main__":
    main()
