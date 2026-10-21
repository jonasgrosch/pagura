#!/usr/bin/env python3
"""Audit all38 fixed color-reuse cells; experimental retention is a separate decision."""
import sys

import color_reuse as experiment
from evaluate_n16_output import ROOT, audit, load, require


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("pilot-001", "confirmation-001"),
            "usage: evaluate_color_reuse.py pilot-001|confirmation-001")
    results = ROOT / "benchmarks/results"
    plan = load(results / "color-reuse-plan.json")
    bindings = load(results / "color-reuse-bindings.json")
    require(plan["status"] == "frozen-before-pilot", "plan not frozen before measurement")
    expected = experiment.baseline_commit()
    require(plan["baseline_commit"] == expected, "plan baseline differs from driver")
    require(bindings["identities"]["baseline"]["commit"] == expected, "baseline identity differs")
    experiment.require_baseline(bindings["identities"]["baseline"]["root"])
    audit(experiment_module=experiment, experiment_name="color-reuse", stem="color-reuse",
          local_name="color-reuse", run=sys.argv[1], expected_case_count=19, expected_cell_count=38,
          target_case=experiment.TARGET_CASE, target_operation=experiment.TARGET_OPERATION,
          expected_guard_mode="strict",
          expected_harness_sources={
              "benchmarks/color_reuse.py", "benchmarks/color_reuse_fixtures.py",
              "benchmarks/paired_render.py",
              "benchmarks/results/sources/analytic19-conversion/manifest.json",
              "benchmarks/results/sources/analytic19-conversion/references.json",
          }, audit_entrypoint="benchmarks/evaluate_color_reuse.py")


if __name__ == "__main__":
    main()
