#!/usr/bin/env python3
"""Collect all76 fixed side/cell quality checks before any color-reuse timing trial."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys

import color_reuse as experiment
import color_reuse_fixtures as fixtures
from paired_render import ROOT, MAX_BYTES, MAX_RETAINED, expected_image, harness_identity, source_identity
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.measurement import run_process


def main(*, experiment_module=experiment, experiment_name="color-reuse", entrypoint=None):
    selected = experiment_module
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bindings", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not os.environ.get("PAGURA_RUN_ID") or not os.environ.get("PAGURA_RUN_RECEIPT"):
        parser.error("run through the serial coordinator with run ID and receipt")
    raw_bindings = read_bounded_regular(args.bindings, MAX_BYTES)
    bindings = json.loads(raw_bindings)
    identities = bindings["identities"]
    if set(identities) != {"baseline", "candidate"} or Path(identities["candidate"]["root"]).resolve() != ROOT:
        raise ValueError("wrong preflight source identities")
    selected.require_baseline(identities["baseline"]["root"])
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "experiment": experiment_name, "purpose": "all-cell independent quality preflight",
              "status": "incomplete", "admission": False, "expected_outputs": 76, "samples": [],
              "identities": identities, "bindings_sha256": hashlib.sha256(raw_bindings).hexdigest(),
              "run_id": os.environ["PAGURA_RUN_ID"], "receipt": os.environ["PAGURA_RUN_RECEIPT"],
              "artifacts": {}, "failures": [], "paired_output_differences": [],
              "scope": "No latency analysis; all numeric/render failures retained without dropping cells. Cancellation or invalid provenance stops the run."}
    stored = 0

    def retain(relative, raw):
        nonlocal stored
        if stored + len(raw) > MAX_RETAINED:
            raise RuntimeError("preflight retention budget exhausted")
        path = output / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)
        stored += len(raw)
        report["artifacts"][relative] = {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}

    def checkpoint():
        report["stored_bytes"] = stored
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    paths = (Path(__file__).resolve(), Path(selected.__file__).resolve(), Path(fixtures.__file__).resolve(),
             ROOT / "benchmarks/paired_render.py", fixtures.MANIFEST, fixtures.REFERENCES)
    if entrypoint is not None:
        paths += (Path(entrypoint).resolve(), Path(experiment.__file__).resolve())
    frozen_harness = harness_identity(paths)
    report["harness_sha256"] = frozen_harness
    if any(bindings.get("bound_files_sha256", {}).get(name) != digest
           for name, digest in frozen_harness.items()):
        raise ValueError("preflight/shared/reference sources differ from frozen bindings")

    def check_sources():
        for side, identity in identities.items():
            actual = source_identity(identity["root"])
            if actual != {key: identity[key] for key in ("root", "commit", "source_sha256")}:
                raise RuntimeError("frozen source changed: " + side)
            if hash_regular_file(Path(identity["binary"])) != identity["binary_sha256"]:
                raise RuntimeError("frozen binary changed: " + side)
        if harness_identity(paths) != frozen_harness:
            raise RuntimeError("preflight/fixture harness changed")

    try:
        check_sources()
        retain("bindings.json", raw_bindings)
        inputs, oracles = {}, {}
        for name in selected.CASES:
            pdf, oracle = selected.fixture(name)
            retain("inputs/" + name + ".pdf", pdf)
            retain("oracles/" + name + ".ppm", oracle)
            inputs[name], oracles[name] = pdf, expected_image(oracle, "cold")
        policy = {"timeout_seconds": 120, "maximum_output_bytes": MAX_BYTES, "measure_rss": False}
        for name, mode, operation in selected.CELLS:
            pair = {}
            for side in ("baseline", "candidate"):
                identity, expected = identities[side], oracles[name]
                pdf_path = output / "inputs" / (name + ".pdf")
                if read_bounded_regular(pdf_path, MAX_BYTES) != inputs[name]:
                    raise RuntimeError("owned preflight input changed")
                if hash_regular_file(Path(identity["binary"])) != identity["binary_sha256"]:
                    raise RuntimeError("frozen binary changed: " + side)
                path = output / "pending.rgba"
                path.unlink(missing_ok=True)
                command = [identity["binary"], str(pdf_path), mode, operation, str(path), "72"]
                sample = {"case": name, "mode": mode, "operation": operation, "side": side,
                          "command": command, "status": "failed"}
                report["samples"].append(sample)
                process, diagnostic = run_process(command, policy)
                sample.update(process=process, diagnostic=diagnostic)
                raw = None
                if path.exists():
                    try:
                        raw = read_bounded_regular(path, MAX_BYTES)
                    except (OSError, ValueError) as error:
                        sample["error"] = f"unreadable output: {error}"
                if raw is not None:
                    relative = f"outputs/{name}/{mode}/{side}.rgba"
                    retain(relative, raw)
                    sample["output"] = relative
                    sample["output_sha256"] = hashlib.sha256(raw).hexdigest()
                    pair[side] = sample["output_sha256"]
                if process.get("status") == "timeout" or process.get("exit_code") != 0:
                    sample["error"] = "renderer process did not succeed"
                elif raw is None:
                    sample.setdefault("error", "missing output")
                else:
                    try:
                        actual = normalize(raw)
                        header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
                        alpha_ok = raw.startswith(header) and raw[len(header) + 3::4] == b"\xff" * (expected[0] * expected[1])
                        sample.update(dimensions=[actual[0], actual[1]], oracle_equal=actual == expected,
                                      opaque_alpha_equal=alpha_ok)
                        if actual[:2] == expected[:2] and len(actual[2]) == len(expected[2]):
                            errors = [abs(a - b) for a, b in zip(actual[2], expected[2])]
                            sample.update(max_channel_error=max(errors, default=0),
                                          different_channels=sum(error != 0 for error in errors))
                        if actual == expected and alpha_ok:
                            sample["status"] = "passed"
                        else:
                            sample["error"] = "independent RGB or opaque alpha mismatch"
                    except (OSError, ValueError) as error:
                        sample["error"] = f"invalid output: {error}"
                if sample["status"] != "passed":
                    report["failures"].append({key: sample.get(key) for key in ("case", "mode", "side", "error")})
                checkpoint()
            if len(pair) == 2 and pair["baseline"] != pair["candidate"]:
                report["paired_output_differences"].append({"case": name, "mode": mode, "sha256": pair})
        check_sources()
        if len(report["samples"]) != 76:
            raise RuntimeError("preflight matrix incomplete")
        report["status"] = "passed-all-76" if not report["failures"] and not report["paired_output_differences"] else "blocked-quality"
    except BaseException as error:
        report.update(status="invalid-or-incomplete", fatal_error=f"{type(error).__name__}: {error}")
        raise
    finally:
        checkpoint()
    print(json.dumps({"status": report["status"], "failures": report["failures"], "report": str(output / "report.json")}, indent=2))
    return 0 if report["status"] == "passed-all-76" else 1


if __name__ == "__main__":
    raise SystemExit(main())
