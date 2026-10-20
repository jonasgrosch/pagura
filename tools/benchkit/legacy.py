"""Original independently oracled end-to-end process track."""
import json
import os
import platform
import random
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from .runner_evidence import coordination_pointer
from .config import ROOT, POLICY_PATH, MATRIX_PATH, read_json, source_digest, sha
from .corpus import render_cases, manifest_digest
from .engines import engines, identify, command_for
from .images import ppm
from .measurement import measure
from .artifacts import ArtifactStore
from .gate import gate
from .reports import local_report_path, markdown


def run(args):
    policy = read_json(POLICY_PATH)
    requested = args.engines.split(",")
    if "pagura" not in requested or len(set(requested)) != len(requested):
        raise ValueError("include pagura exactly once, and no duplicate engines")
    bases = engines()
    if set(requested) - bases.keys():
        raise ValueError("unknown engine")
    subprocess.run(["cargo", "build", "--release", "--locked", "--offline", "-p", "pagura"], cwd=ROOT, check=True)
    identities = {name: identify(name, bases[name]) for name in requested}
    cases = render_cases()
    for case in cases:
        for path_key, hash_key in [("path", "sha256"), ("oracle", "oracle_sha256")]:
            if sha((ROOT / case[path_key]).read_bytes()) != case[hash_key]:
                raise ValueError(f"corpus changed: {case['id']}")
    git = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=ROOT)
    rustc = subprocess.check_output(["rustc", "-Vv"], text=True, cwd=ROOT).strip()
    hardware = {}
    if platform.system() == "Darwin":
        for field in ["machdep.cpu.brand_string", "hw.memsize", "hw.physicalcpu", "hw.logicalcpu"]:
            value = subprocess.run(["sysctl", "-n", field], capture_output=True, text=True)
            hardware[field] = value.stdout.strip() if value.returncode == 0 else "unavailable"
    report = {"schema": 1, "created_at": datetime.now(timezone.utc).isoformat(),
              "suite": args.suite, "operation": "render-e2e", "samples_requested": args.samples,
              "warmups": args.warmups, "seed": args.seed, "source_sha256": source_digest(),
              "commit": git.stdout.strip() if git.returncode == 0 else None,
              "policy_sha256": sha(POLICY_PATH.read_bytes()), "manifest_sha256": manifest_digest(),
              "matrix_sha256": sha(MATRIX_PATH.read_bytes()), "settings": policy["settings"],
              "runner": {"system": platform.platform(), "machine": platform.machine(), "processor": platform.processor(),
                         "hardware": hardware, "rustc": rustc, "python": sys.version,
                         "cpu_count": os.cpu_count(), "controlled": False,
                         "coordination": coordination_pointer(),
                         "note": "shared desktop; no thermal/power/idle attestation; OS filesystem cache warm after warmups"},
              "engine_identities": identities, "cells": []}
    artifacts = ArtifactStore.for_report(report, policy)
    rng = random.Random(args.seed)
    jobs = [(case, engine) for case in cases for engine in requested if engine != "pagura"]
    rng.shuffle(jobs)
    output_dir = ROOT / "benchmarks/results/local"
    output_dir.mkdir(parents=True, exist_ok=True)
    report_path = local_report_path(args.output, "latest.json")
    report_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="run-", dir=output_dir) as directory:
        for case, engine in jobs:
            snapshot = artifacts.snapshot(case)
            invocation_case = {**case, "path": str(snapshot)} if snapshot is not None else None
            expected = ppm((ROOT / case["oracle"]).read_bytes())
            cell = {"case": case["id"], "engine": engine, "operation": "render-e2e",
                    "pdf_sha256": case["sha256"], "oracle_pixels_sha256": sha(expected[2]),
                    "status": "unavailable", "samples": [], "warmup_samples": [], "warmup_failures": [], "commands": {}}
            report["cells"].append(cell)
            if snapshot is None:
                cell.update(status="invalid-evidence", reason="PDF snapshot unavailable; artifact evidence incomplete")
            elif all(identities[n]["status"] == "available" for n in ["pagura", engine]):
                cell["status"] = "measured"
                for pair in range(-args.warmups, args.samples):
                    order = ["pagura", engine]
                    rng.shuffle(order)
                    sample = {"pair": pair, "order": order}
                    for name in order:
                        output = Path(directory) / f"{name}.ppm"
                        output.unlink(missing_ok=True)
                        command = command_for(name, bases[name], invocation_case, output)
                        cell["commands"][name] = command
                        context = artifacts.freeze_execution(cases=[case], snapshot=snapshot, engine=name,
                            comparison_engine=engine, pair=pair, order=order, command=command,
                            output_paths={case.get("page", 0): output})
                        result = measure(command, output, expected, policy,
                                         artifact_store=artifacts, execution_context=context)
                        sample["pagura" if name == "pagura" else "baseline"] = result
                    if pair >= 0:
                        cell["samples"].append(sample)
                    else:
                        cell["warmup_samples"].append(sample)
                        if any(sample[k]["status"] != "ok" for k in ["pagura", "baseline"]):
                            cell["warmup_failures"].append(sample)
            report_path.write_text(json.dumps(report, indent=2) + "\n")
            bad = sum(s[k]["status"] != "ok" for s in cell["samples"] for k in ["pagura", "baseline"])
            print(f"{case['id']} / {engine}: {cell['status']}, {bad} incorrect/failed measured outputs", flush=True)
    artifacts.finish()
    verdict = gate(report)
    report["gate"] = verdict
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    report_path.with_suffix(".md").write_text(markdown(report))
    print(f"Report: {report_path}\nRelease gate: {'PASS' if verdict['pass'] else 'BLOCKED'}")

