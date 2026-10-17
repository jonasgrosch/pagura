"""Local raw evidence paths and reviewable digest-bound summaries."""
import json
import statistics
import subprocess
import platform
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path

from .config import ROOT, POLICY_PATH, MATRIX_PATH, source_digest, sha
from .corpus import manifest_digest


def markdown(report):
    lines = ["# Pagura exploratory benchmark", "", f"Recorded: {report['created_at']}", "",
             "**Release gate: BLOCKED.** These are bootstrap end-to-end process measurements,",
             "including runtime startup, wrapper work, file I/O and PPM output. They are not",
             "a comparison of warm rendering cores and do not establish general 20% superiority.", "",
             "| Fixture | Competitor | Correct pairs | Pagura median ms | Baseline median ms | Ratio |", "|---|---|---:|---:|---:|---:|"]
    for cell in sorted(report["cells"], key=lambda c: (c["case"], c["engine"])):
        samples = cell["samples"]
        correct = sum(s["pagura"]["status"] == s["baseline"]["status"] == "ok" for s in samples)
        if samples:
            a, b = (statistics.median(s[k]["elapsed_ns"] for s in samples) / 1e6 for k in ["pagura", "baseline"])
            ratio = f"{a / b:.3f}" if correct == len(samples) else "invalid output"
            lines.append(f"| {cell['case']} | {cell['engine']} | {correct}/{len(samples)} | {a:.3f} | {b:.3f} | {ratio} |")
        else:
            lines.append(f"| {cell['case']} | {cell['engine']} | unavailable | — | — | — |")
    lines += ["", "Gate blockers:", ""] + ["- " + reason for reason in report["gate"]["reasons"]]
    lines += ["", "Exact versions, executable/adapter hashes, corpus hashes, randomized order, peak RSS and every raw sample are in the adjacent JSON."]
    return "\n".join(lines) + "\n"


def local_report_path(output, default="conversion.json"):
    """Raw samples belong exclusively to the ignored local evidence directory."""
    directory = (ROOT / "benchmarks/results/local").resolve()
    path = Path(output).resolve() if output else directory / default
    if not path.is_relative_to(directory) or path.suffix != ".json":
        raise ValueError("raw reports must be JSON under benchmarks/results/local/")
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def conversion_markdown(report):
    """Compact shareable summary without raw samples or private document paths."""
    def escape(value):
        return str(value).replace("|", "\\|").replace("\n", " ").replace("\r", " ")

    lines = ["# Pagura " + report["operation"] + " benchmark", "", "Release gate: **BLOCKED** (exploratory evidence).",
             "Unverified cells are investigation data. Pixel agreement does not prove correctness.",
             ("Each page is converted in a new process; engine contribution = open + render."
              if report.get("settings", {}).get("page_process") != "one open per document" else
              "One process/open per document and DPI. Document engine contribution = open + sum(page render)."), "",
             "| Document | Page (1-based) | DPI | Baseline | Verification | Process ratio | Engine ratio |",
             "|---|---:|---:|---|---|---:|---:|"]
    summary_cells = report.get("document_cells") or report["cells"]
    for cell in summary_cells:
        stats = cell["statistics"]
        def ratio(metric):
            value = stats[metric]
            return f"{value['median_ratio']:.3f} (upper {value['median_upper']:.3f}; p95 upper {value['p95_upper']:.3f})" if value["status"] == "measured" else value["status"]
        verification = cell.get("verification", "document; see per-page verification") + ("; warmup failure" if cell["warmup_failures"] else "")
        if any(sample[key]["status"] not in ("ok", "unverified") for sample in cell["samples"] for key in ("pagura", "baseline")):
            verification += "; failed output/timing"
        lines.append(f"| {escape(cell['case'])} | {cell['page'] + 1 if 'page' in cell else 'all selected'} | {cell['dpi']} | {escape(cell['engine'])} | {verification} | {ratio('elapsed_ns')} | {ratio('engine_ns')} |")
    lines += ["", "Input manifest SHA-256: `" + report["input_sha256"] + "`",
              "Measured source SHA-256: `" + report["source_sha256"] + "`",
              "Policy SHA-256: `" + report["policy_sha256"] + "`",
              "Full local report contains versions/artifact hashes, paired order, stage samples, peak RSS, errors and agreement metrics."]
    return "\n".join(lines) + "\n"


def write_summary(report_path, destination):
    """Produce a new Markdown summary with exact input and raw-report digests."""
    report_path = Path(report_path)
    data = report_path.read_bytes()
    report = json.loads(data)
    destination = Path(destination).resolve()
    summaries = (ROOT / "benchmarks/results").resolve()
    if not destination.is_relative_to(summaries) or destination.suffix != ".md":
        raise ValueError("summary must be Markdown under benchmarks/results/")
    text = conversion_markdown(report) if report.get("operation") in ("conversion", "regression") else markdown(report)
    text += "\nRaw report SHA-256: `" + sha(data) + "`\n"
    text += "Generated manifest SHA-256: `" + report["manifest_sha256"] + "`\n"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("x") as output:
        output.write(text)


def new_report(identities, samples, warmups, seed, input_digest, operation, revision, *, process_per_page):
    """Common provenance; checkpoints and completed exploratory reports fail closed."""
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=False)
    report = {"schema": 2, "commit": commit.stdout.strip() if commit.returncode == 0 else None,
              "toolchain": tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"], "created_at": datetime.now(timezone.utc).isoformat(), "suite": "smoke",
              "operation": operation, "samples_requested": samples, "warmups": warmups, "seed": seed,
              "source_sha256": source_digest(), "policy_sha256": sha(POLICY_PATH.read_bytes()),
              "matrix_sha256": sha(MATRIX_PATH.read_bytes()), "manifest_sha256": manifest_digest(),
              "input_sha256": input_digest, "revision": revision, "engine_identities": identities,
              "settings": {"background": "white", "normalization": "RGB8 over white", "page_process": "reopen per page" if process_per_page else "one open per document"},
              "runner": {"system": platform.platform(), "machine": platform.machine(), "python": sys.version,
                         "controlled": False, "note": "no idle/thermal/power attestation; process and stage tracks separate"},
              "cells": [], "agreement": [],
              "gate": {"pass": False, "reasons": ["incomplete exploratory checkpoint"]}}
    report["document_cells"] = []
    return report
