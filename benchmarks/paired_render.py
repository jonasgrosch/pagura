"""Shared loop for the two small native retained-render experiments.

No renderer policy or admission evaluator lives here. Each entry point supplies
its fixed fixtures, cells, scope and (when required) frozen phase settings.
"""
import argparse
import hashlib
import json
import os
import platform
import random
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.measurement import run_process
from benchkit.statistics import ratio_bounds

MAX_BYTES = 64 * 1024 * 1024
MAX_RETAINED = 512 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def harness_identity(paths):
    """Bind Python entry/shared/fixture files omitted by source_identity."""
    return {str(path.resolve().relative_to(ROOT)): hash_regular_file(path)
            for path in paths}


def source_identity(root):
    root = Path(root).resolve()
    paths = set()
    for name in ("crates", "tools", "benchmarks/adapters"):
        paths.update(path for path in (root / name).rglob("*") if path.is_file()
                     and "target" not in path.parts and "__pycache__" not in path.parts)
    paths.update(path for path in (root / "corpus/manifests").glob("*.json"))
    paths.update(root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"))
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes())
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    return {"root": str(root), "commit": commit, "source_sha256": digest.hexdigest()}


def expected_image(oracle, operation):
    width, height, pixels = normalize(oracle)
    if operation != "tile":
        return width, height, pixels
    w, h = min(width, 256), min(height, 256)
    return w, h, b"".join(pixels[row * width * 3:row * width * 3 + w * 3] for row in range(h))


def run_experiment(entrypoint, *, description, experiment, cases, cells, target_case,
                   fixture, target_scope, limitations, fixture_sources=(), phase_plan=None,
                   target_operation="full"):
    """Run the fixed matrix once, retaining every warmup and measured output."""
    entrypoint = Path(entrypoint).resolve()
    harness_sources = (entrypoint, Path(__file__).resolve(), *map(Path, fixture_sources))
    parser = argparse.ArgumentParser(description=description)
    for side in ("baseline", "candidate"):
        parser.add_argument("--" + side, required=True, type=Path)
        parser.add_argument("--" + side + "-source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="new private evidence directory")
    parser.add_argument("--pairs", type=int)
    parser.add_argument("--warmups", type=int)
    parser.add_argument("--phase", choices=("pilot", "confirmation"), default="pilot")
    parser.add_argument("--seed", type=int)
    args = parser.parse_args()
    if phase_plan is None:
        for name, default in (("pairs", 5), ("warmups", 5), ("seed", 42017)):
            if getattr(args, name) is None:
                setattr(args, name, default)
    else:
        for name, value in phase_plan[args.phase].items():
            if getattr(args, name) not in (None, value):
                parser.error(f"{args.phase} fixes --{name} at {value}")
            setattr(args, name, value)
    if not 2 <= args.pairs <= 1000 or not 0 <= args.warmups <= 100:
        parser.error("pairs must be 2..1000; warmups 0..100")
    if args.phase == "confirmation" and (args.pairs < 100 or args.warmups < 5):
        parser.error("confirmation needs at least100 fresh pairs and five warmups")
    roots = {side: getattr(args, side + "_source").resolve() for side in ("baseline", "candidate")}
    # Imports above come from this checkout; bind those exact tools/helpers to
    # the recorded candidate source identity, even when invoked from elsewhere.
    if roots["candidate"] != ROOT:
        parser.error("--candidate-source must be the benchmark driver's checkout")
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    report_path = directory / "report.json"
    binaries = {side: getattr(args, side).resolve() for side in roots}
    identities = {side: {**source_identity(root), "binary": str(binaries[side]),
                         "binary_sha256": hash_regular_file(binaries[side])} for side, root in roots.items()}
    example = Path("crates/pagura-render/examples/scene_index_profile.rs")
    if (roots["baseline"] / example).read_bytes() != (roots["candidate"] / example).read_bytes():
        raise ValueError("baseline and candidate harness sources differ")
    harness = harness_identity(harness_sources)
    report = {"schema": 1, "experiment": experiment, "phase": args.phase,
        "status": "running", "admission": "not evaluated; pilot can never accept",
        "identities": identities, "driver_sha256": harness[str(entrypoint.relative_to(ROOT))],
        "harness_sources_sha256": harness, "phase_plan": phase_plan,
        "example_sha256": sha((roots["candidate"] / example).read_bytes()),
        "pairs": args.pairs, "warmups": args.warmups, "seed": args.seed, "dpi": 72,
        "scope": "native one-thread retained full-validation API; hashed/warm filesystem; not host or WASM",
        "target_scope": target_scope,
        "environment": {"platform": platform.platform(), "machine": platform.machine(),
                        "python": sys.version, "run_id": os.environ.get("PAGURA_RUN_ID"),
                        "run_receipt": os.environ.get("PAGURA_RUN_RECEIPT"),
                        "rust_toolchain": (roots["candidate"] / "rust-toolchain.toml").read_text()},
        "limitations": ["build commands and binary/source correspondence are trusted coordinator records",
                        *limitations,
                        "sample count does not prove adequate p95 precision"],
        "thresholds": {"target_median_upper": .95, "target_p95_upper": 1.0,
                       "protected_median_upper": 1.02, "protected_p95_upper": 1.02,
                       "peak_rss_ratio_max": 1.0},
        "statistics": {"method": "paired percentile bootstrap", "resamples": 5000,
                       "one_sided_confidence": .95, "seed": 42017,
                       "scope": "marginal estimates; temporal dependence and p95 precision need review"},
        "declared_cells": [{"case": name, "mode": mode, "operation": operation,
                            "role": "target" if name == target_case and operation == target_operation else "protected"}
                           for name, mode, operation in cells],
        "inputs": {}, "cells": []}
    stored = 0

    def retain(relative, data):
        nonlocal stored
        path = directory / relative
        if path.exists():
            if read_bounded_regular(path, len(data)) != data:
                raise ValueError("retained evidence changed")
            return
        if stored + len(data) > MAX_RETAINED:
            raise ValueError("experiment retention budget exhausted")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o400)
        stored += len(data)

    def checkpoint():
        report["stored_bytes"] = stored
        report_path.write_text(json.dumps(report, indent=2) + "\n")

    policy = {"timeout_seconds": 120, "maximum_output_bytes": MAX_BYTES, "measure_rss": True}
    rng = random.Random(args.seed)
    try:
        oracles = {}
        for name in cases:
            pdf, oracle = fixture(name)
            retain(f"inputs/{name}.pdf", pdf)
            retain(f"oracles/{name}.ppm", oracle)
            report["inputs"][name] = {"pdf_sha256": sha(pdf), "oracle_sha256": sha(oracle)}
            oracles[name] = oracle
        ordered_cells = list(cells)
        rng.shuffle(ordered_cells)
        for name, mode, operation in ordered_cells:
            cell = {"case": name, "mode": mode, "operation": operation,
                    "role": "target" if name == target_case and operation == target_operation else "protected", "pairs": []}
            report["cells"].append(cell)
            expected = expected_image(oracles[name], operation)
            for pair in range(-args.warmups, args.pairs):
                order = ["baseline", "candidate"]
                rng.shuffle(order)
                sample = {"pair": pair, "order": order, "started_monotonic_ns": time.monotonic_ns()}
                cell["pairs"].append(sample)
                for side in order:
                    pdf = directory / "inputs" / (name + ".pdf")
                    if sha(read_bounded_regular(pdf, MAX_BYTES)) != report["inputs"][name]["pdf_sha256"]:
                        raise ValueError("input changed")
                    if hash_regular_file(binaries[side]) != identities[side]["binary_sha256"]:
                        raise ValueError("binary changed")
                    output = directory / "pending.rgba"
                    output.unlink(missing_ok=True)
                    command = [str(binaries[side]), str(pdf), mode, operation, str(output), "72"]
                    process, diagnostic = run_process(command, policy)
                    record = {"command": command, "process": process, "diagnostic": diagnostic}
                    sample[side] = record
                    if process["exit_code"] != 0 or process["status"] == "timeout":
                        raise ValueError("sample failed; exact diagnostic retained")
                    stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
                    if len(stages) != 1 or not isinstance(stages[0], dict) or any(type(stages[0].get(key)) is not int or stages[0][key] <= 0
                        for key in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")):
                        raise ValueError("missing or invalid stage timings")
                    record["stages"] = stages[0]
                    stages = stages[0]
                    if (stages.get("schema") != 1 or stages.get("operation") != operation
                            or stages.get("accuracy") != mode or stages.get("dpi") != 72
                            or stages.get("width") != expected[0] or stages.get("height") != expected[1]
                            or stages["metric_ns"] != stages["total_ns" if operation == "cold" else "render_ns"]
                            or stages["total_ns"] < stages["open_ns"] + stages["compile_ns"] + stages["render_ns"]):
                        raise ValueError("stage identity or timing relationship mismatch")
                    raw = read_bounded_regular(output, MAX_BYTES)
                    blob = "blobs/" + sha(raw) + ".rgba"
                    retain(blob, raw)
                    actual = normalize(raw)
                    header = f"RGBA8 {expected[0]} {expected[1]} premultiplied\n".encode()
                    if not raw.startswith(header) or raw[len(header) + 3::4] != b"\xff" * (expected[0] * expected[1]):
                        raise ValueError("expected premultiplied RGBA8 over opaque white background")
                    record.update(blob=blob, output_sha256=sha(raw), pixels_sha256=sha(actual[2]),
                                  width=actual[0], height=actual[1], oracle_equal=actual == expected)
                    if actual != expected:
                        raise ValueError("independent integer-rectangle oracle mismatch")
                    output.unlink()
                if sample["baseline"]["output_sha256"] != sample["candidate"]["output_sha256"]:
                    raise ValueError("same-policy baseline/candidate byte mismatch")
                sample["finished_monotonic_ns"] = time.monotonic_ns()
                checkpoint()
            print(f"completed {len(report['cells'])}/{len(cells)}: {name}/{mode}/{operation}", flush=True)
        for side, root in roots.items():
            if (source_identity(root) != {key: identities[side][key] for key in ("root", "commit", "source_sha256")}
                    or hash_regular_file(binaries[side]) != identities[side]["binary_sha256"]):
                raise ValueError("source or binary changed during run")
        if harness_identity(harness_sources) != harness:
            raise ValueError("benchmark entry/shared/fixture sources changed during run")
        # Statistical work follows all timed samples; no interim admission.
        for cell in report["cells"]:
            pairs = [sample for sample in cell["pairs"] if sample["pair"] >= 0]
            cell["latency"] = ratio_bounds([sample["candidate"]["stages"]["metric_ns"] for sample in pairs],
                                          [sample["baseline"]["stages"]["metric_ns"] for sample in pairs])
            rss = {side: [sample[side]["process"].get("peak_rss_bytes") for sample in pairs] for side in roots}
            cell["peak_rss_ratio"] = (max(rss["candidate"]) / max(rss["baseline"])
                if all(type(value) is int and value > 0 for values in rss.values() for value in values) else None)
        report["status"] = "complete-exploratory"
    except BaseException as error:
        report.update(status="invalid-or-incomplete", error=str(error))
        raise
    finally:
        checkpoint()
    print(report_path)
