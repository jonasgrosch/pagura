#!/usr/bin/env python3
"""Record direct-root composition errors using a prebuilt public-API driver.

Run serially under tools/runner.py. This diagnostic deliberately records an
accuracy failure without treating it as successful admission. Its exit status
reports execution/integrity errors; inspect every row's within_contract field.
There are no speed or memory claims, repetitions, retries or changed bounds.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from opaque_precision_fixtures import CASES, fixture
import opaque_precision_fixtures


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    cases = {}
    for name in CASES:
        pdf, oracle, bounds = fixture(name)
        input_path, oracle_path = output / (name + ".pdf"), output / (name + ".ppm")
        input_path.write_bytes(pdf)
        oracle_path.write_bytes(oracle)
        cases[name] = {"input_sha256": sha(input_path), "oracle_sha256": sha(oracle_path), **bounds}
    sources = [Path(__file__).resolve(), Path(opaque_precision_fixtures.__file__).resolve(),
               Path(__file__).resolve().parents[1] / "tools/fixturegen/pdf.py"]
    plan = {
        "schema": 1, "kind": "independent-accuracy-diagnostic", "admission": False,
        "binary": str(binary), "binary_sha256": sha(binary),
        "source_hashes": {str(path): sha(path) for path in sources},
        "cases": cases, "modes": ["exact", "competitive"], "operations": ["full", "tile"],
        "limitations": ["Driver source/build correspondence requires the separately bound build evidence.",
                        "The one-pixel tile checks the API entry point, not multi-tile seams.",
                        "Driver uses default open options and does not emit diagnostics or a dispatch trace.",
                        "Exact one-level bounds are applied to both modes here without inventing a Competitive allowance.",
                        "Timings are retained as driver output only; no performance comparison is made."],
    }
    plan_path = output / "plan.json"
    plan_path.write_text(json.dumps(plan, indent=2) + "\n")
    rows = []
    report_path = output / "report.json"
    report = {"schema": 1, "admission": False, "plan_sha256": sha(plan_path),
              "status": "running", "rows": rows}
    try:
        for name in CASES:
            bounds = cases[name]
            for mode in plan["modes"]:
                rendered = []
                for operation in plan["operations"]:
                    stem = f"{name}-{mode}-{operation}"
                    target = output / (stem + ".rgba")
                    command = [str(binary), str(output / (name + ".pdf")), mode, operation, str(target), "72"]
                    with (output / (stem + ".stdout")).open("xb") as stdout, (output / (stem + ".stderr")).open("xb") as stderr:
                        process = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=60, check=False)
                    row = {"case": name, "mode": mode, "operation": operation,
                           "command": command, "exit_code": process.returncode}
                    rows.append(row)
                    if process.returncode != 0:
                        raise RuntimeError("renderer failed: " + stem)
                    raw = target.read_bytes()
                    header, rgba = raw.split(b"\n", 1)
                    if header != b"RGBA8 1 1 premultiplied" or len(rgba) != 4:
                        raise ValueError("unexpected output extent/format")
                    row.update(rgba=list(rgba), output_sha256=hashlib.sha256(raw).hexdigest(),
                               within_contract=all(v in bounds["admissible"] for v in rgba[:3]) and rgba[3] == bounds["alpha"])
                    rendered.append(raw)
                if rendered[0] != rendered[1]:
                    raise ValueError("full/tile entry points differ")
        if sha(binary) != plan["binary_sha256"] or any(sha(p) != h for p, h in plan["source_hashes"].items()):
            raise ValueError("binary or harness changed during probe")
        for name, bounds in cases.items():
            if sha(output / (name + ".pdf")) != bounds["input_sha256"] or sha(output / (name + ".ppm")) != bounds["oracle_sha256"]:
                raise ValueError("fixture changed during probe")
        if any(sha(row["command"][4]) != row["output_sha256"] for row in rows):
            raise ValueError("verified output changed during probe")
        report.update(status="completed-diagnostic", all_within_contract=all(row["within_contract"] for row in rows),
                      full_tile_equal=True)
    except BaseException as error:
        report.update(status="execution-failed", error=str(error))
        raise
    finally:
        report["artifacts_sha256"] = {path.name: sha(path) for path in sorted(output.iterdir()) if path.is_file() and path != report_path}
        report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "rows": len(rows), "all_within_contract": report["all_within_contract"]}))


if __name__ == "__main__":
    main()
