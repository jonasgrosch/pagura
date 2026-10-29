"""Pinned reference medians and unpaired comparisons for round-5 re-timing."""
import collections
import hashlib
import json
import math
import statistics

GOOD = ("ok", "unverified")
CONTENT = ("text-heavy", "image-heavy", "vector-heavy", "mixed")
DRIFT_METRICS = ("render_ns", "engine_ns", "elapsed_ns")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def key(cell):
    return cell["case"], cell.get("page"), cell["dpi"], cell["engine"]


def machine_identity(record):
    identity = {name: record.get(name) for name in ("cpu_model", "platform", "architecture")}
    if not all(identity.values()):
        raise ValueError("reference comparison requires CPU model, OS and architecture")
    version = record.get("os_version", {})
    if version.get("status") == "available":
        identity["os_version"] = version["stdout"]
    return identity


def medians(directory, samples):
    """Never filter failed samples or manufacture page-level process timings."""
    groups = {}
    seen = set()
    with (directory / "blocks.jsonl").open() as stream:
        for line in stream:
            block = json.loads(line)
            for engine, run in block["runs"].items():
                sample_key = block["case"], block["dpi"], engine, block["pair"]
                if sample_key in seen:
                    raise ValueError("duplicate engine sample in round-5 blocks")
                seen.add(sample_key)
                for page in [None, *run["pages"]]:
                    cell = {"case": block["case"], "page": None if page is None else page["page"],
                            "dpi": block["dpi"], "engine": engine}
                    group = groups.setdefault(key(cell), {**cell, "runs": [], "warmup_failures": 0})
                    value = run if page is None else page
                    good = run["status"] in GOOD and value["status"] in GOOD
                    if block["pair"] < 0:
                        group["warmup_failures"] += not good
                    else:
                        metrics = dict(value.get("stages") or {})
                        if page is None:
                            metrics.update({name: run.get(name) for name in
                                            ("elapsed_ns", "cpu_user_ns", "cpu_system_ns", "cpu_ns", "peak_rss_bytes")})
                        group["runs"].append((good, metrics))
    result = {}
    for cell_key, group in groups.items():
        runs = group.pop("runs")
        good = len(runs) == samples and all(ok for ok, _ in runs) and not group["warmup_failures"]
        names = set().union(*(values for _, values in runs))
        values = {}
        for name in sorted(names):
            numbers = [values.get(name) for _, values in runs]
            values[name] = (statistics.median(numbers) if good and all(
                isinstance(number, (int, float)) and math.isfinite(number) and number >= 0 for number in numbers) else None)
        result[cell_key] = {**group, "samples": len(runs), "status": "measured" if good else "failed", "metrics": values}
    return result


def protocol_identity(report):
    """The measured RSS floor can vary; all instrumentation settings must match."""
    fields = ("samples_requested", "warmups", "policy_sha256", "measurement_options")
    if any(name not in report for name in fields):
        raise ValueError("missing reference measurement protocol fields")
    identity = {name: report[name] for name in fields}
    identity["measurement_options"] = {name: value for name, value in report["measurement_options"].items()
                                       if name != "rss_floor_bytes"}
    process_hash = report.get("round5", {}).get("harness_sha256", {}).get("benchmarks/process_measurement.py")
    if not process_hash:
        raise ValueError("missing reference process_measurement.py hash")
    identity["process_measurement_sha256"] = process_hash
    return identity


def load_reference(directory, manifest_hash, machine, protocol):
    directory = directory.expanduser().resolve()
    data = {name: (directory / name).read_bytes() for name in ("conversion.json", "machine.json", "engines.json", "status.json")}
    report, machines, identities, status = (json.loads(data[name]) for name in data)
    if status.get("status") != "complete" or status.get("failed_cells") or report.get("completed_jobs") != report.get("planned_jobs"):
        raise ValueError("reference must be a complete successful round-5 run")
    if not report.get("suite", "").startswith("round5-"):
        raise ValueError("reference is not a round-5 run")
    if "retiming" in report:
        raise ValueError("a re-timing run cannot be a reference")
    expected, actual = protocol_identity(protocol), protocol_identity(report)
    for name in expected:
        if actual[name] != expected[name]:
            raise ValueError(f"reference {name} differs from current measurement protocol")
    if report.get("input_sha256") != manifest_hash:
        raise ValueError("reference manifest hash differs")
    if identities != report.get("engine_identities"):
        raise ValueError("reference engine identities disagree with conversion report")
    current = machine_identity(machine)
    observations = machines.get("observations", [])
    if not observations or any(machine_identity(observation) != current for observation in observations):
        raise ValueError("reference CPU model or OS differs from this machine")
    cells = medians(directory, report["samples_requested"])
    if not cells or any(cell["status"] != "measured" for cell in cells.values()):
        raise ValueError("reference contains missing or failed samples")
    provenance = {"run_id": directory.name, "directory": str(directory), "created_at": report["created_at"],
                  "commit": report.get("commit"), "manifest_sha256": manifest_hash,
                  "machine": current, "sha256": {name: digest(value) for name, value in data.items()},
                  "blocks_sha256": file_digest(directory / "blocks.jsonl")}
    return {"provenance": provenance, "report": report, "medians": cells}


def file_digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def check_control_identity(current, reference):
    # Commands and adapter paths can move between checkouts; content must not.
    if (current["version"] != reference["version"] or
            sorted(current["artifacts"].values()) != sorted(reference["artifacts"].values())):
        raise ValueError("control engine identity differs from reference")


def select_controls(reference, count, engine="hayro"):
    """Round-robin categories, rotating DPI each sweep; independent of timings."""
    if not count:
        return []
    buckets = collections.defaultdict(list)
    for cell in reference["report"]["round5"]["selection"]:
        candidate = reference["medians"].get((cell["case"], cell["page"], cell["dpi"], engine))
        if candidate is None or candidate["status"] != "measured" or not candidate["metrics"].get("render_ns"):
            raise ValueError("control reference has missing or failed render timings")
        buckets[cell["content"], cell["dpi"]].append(cell)
    for bucket in buckets.values():
        bucket.sort(key=lambda cell: (cell["case"], cell["page"]))
    dpis = sorted({dpi for _, dpi in buckets})
    chosen = []
    for sweep in range(sum(map(len, buckets.values()))):
        for index, content in enumerate(CONTENT):
            dpi = dpis[(sweep + index) % len(dpis)]
            if buckets[content, dpi]:
                chosen.append(buckets[content, dpi].pop(0))
                if len(chosen) == count:
                    return chosen
    raise ValueError("control count exceeds available reference cells")


def control_drift(controls, current, reference, engine="hayro"):
    metrics = {}
    for metric in DRIFT_METRICS:
        rows, seen = [], set()
        for cell in controls:
            page = cell["page"] if metric == "render_ns" else None
            cell_key = cell["case"], page, cell["dpi"], engine
            if cell_key in seen:
                continue  # Document costs count once, even when controls select several pages.
            seen.add(cell_key)
            new = current.get(cell_key, {}).get("metrics", {}).get(metric)
            old = reference.get(cell_key, {}).get("metrics", {}).get(metric)
            rows.append({**cell, "page": page, "engine": engine,
                         "ratio": new / old if new is not None and old else None})
        ratios = [cell["ratio"] for cell in rows]
        median = statistics.median(ratios) if ratios and all(value is not None for value in ratios) else None
        metrics[metric] = {"cells": rows, "median_ratio": median,
                           "status": "not-requested" if not rows else "incomplete" if median is None else
                                     "drifted" if not 0.97 <= median <= 1.03 else "stable"}
    statuses = {value["status"] for value in metrics.values()}
    status = ("not-requested" if not controls else "incomplete" if "incomplete" in statuses else
              "drifted" if "drifted" in statuses else "stable")
    return {"engine": engine, "metrics": metrics, "status": status}


def comparison(selection, current, reference, rerun):
    """Median of per-cell ratios, with per-cell best chosen before aggregation."""
    old = reference["medians"] if reference else {}
    names = sorted({cell[3] for cell in old} | set(rerun))
    competitors = [name for name in names
                   if name not in ("pagura", "pagura-x86-64-v3", "v8-wasi", "wasmtime")]
    combined = {cell_key: value for cell_key, value in old.items() if cell_key[3] not in rerun}
    # Controls are diagnostics; they never replace the reference's competitor medians.
    combined.update({cell_key: value for cell_key, value in current.items() if cell_key[3] in rerun})
    rows = []
    for metric, page_level in (("render_ns", True), ("engine_ns", False), ("elapsed_ns", False)):
        groups = collections.defaultdict(set)
        for cell in selection:
            groups[cell["content"], cell["dpi"]].add((cell["case"], cell["page"] if page_level else None, cell["dpi"]))
        for (content, dpi), cell_keys in sorted(groups.items()):
            ratios = collections.defaultdict(list)
            for prefix in sorted(cell_keys, key=str):
                def value(engine, source=combined):
                    return source.get((*prefix, engine), {}).get("metrics", {}).get(metric)
                pagura = value("pagura")
                applicable = [engine for engine in competitors if metric == "elapsed_ns" or engine != "poppler"]
                available = [value(engine) for engine in applicable if value(engine)]
                for engine in names:
                    if engine != "pagura":
                        denominator = value(engine)
                        ratios[engine].append(pagura / denominator if pagura is not None and denominator else None)
                ratios["best"].append(pagura / min(available) if pagura is not None and available and
                                      len(available) == len(applicable) else None)
                previous = value("pagura", old)
                ratios["reference_pagura"].append(pagura / previous if pagura is not None and previous else None)
            rows.append({"metric": metric, "content": content, "dpi": dpi, "cells": len(cell_keys),
                         "ratios": {name: statistics.median(values) if all(value is not None for value in values) else None
                                    for name, values in ratios.items()}})
    return {"engines": names, "engine_sources": {name: "measured" if name in rerun else "reference" for name in names},
            "rows": rows}


def markdown(result, drift, provenance):
    lines = ["# Round 5 re-timing", "", "Exploratory, unpaired ratios of medians; lower is faster. No release claim.", ""]
    if provenance:
        lines += [f"Reference: `{provenance['run_id']}`; hashes and machine identity in `reference.json`.", ""]
    status = drift["status"]
    lines += [f"**Comparison: {status.upper()}.**" + (" Median control drift exceeds ±3%." if status == "drifted" else ""), ""]
    if status != "stable":
        lines += ["**NOT A RESULT.** Comparison ratios withheld until all control metrics are stable.", ""]
    for source in ("measured", "reference"):
        engines = [name for name, origin in result["engine_sources"].items() if origin == source]
        if engines:
            lines += [f"{source.capitalize()} engines: " + ", ".join(engines) + ".", ""]
    if status == "stable":
        names = [name for name in result["engines"] if name != "pagura"]
        for metric in DRIFT_METRICS:
            lines += [f"## {metric}", "", "Category | DPI | Cells | " + " | ".join(f"Pagura ÷ {name}" for name in [*names, "best", "reference Pagura"]),
                      "--- | ---: | ---: | " + " | ".join(["---:"] * (len(names) + 2))]
            for row in result["rows"]:
                if row["metric"] == metric:
                    values = [row["ratios"].get(name) for name in [*names, "best", "reference_pagura"]]
                    lines.append(f"{row['content']} | {row['dpi']} | {row['cells']} | " + " | ".join("unavailable" if value is None else f"{value:.4f}" for value in values))
            lines.append("")
    lines += ["Render cells are pages. Engine/process cells are complete document/DPI jobs; open is counted once.",
              "Each category shows the median of its cell ratios. Best is the fastest competitor per cell, excluding Pagura's WASM and x86-64-v3 builds.",
              "Poppler has process timings only. Missing or failed measurements suppress ratios; samples are never filtered.",
              "Control samples are diagnostic and do not replace stored competitor medians.", "", "## Control drift", ""]
    for metric, values in drift["metrics"].items():
        median = values["median_ratio"]
        ratio = "unavailable" if median is None else f"{median:.4f}"
        lines += [f"{metric}: median new ÷ reference = {ratio}; {values['status']}.", ""]
        for cell in values["cells"]:
            ratio = "unavailable" if cell["ratio"] is None else f"{cell['ratio']:.4f}"
            page = "document" if cell["page"] is None else f"page {cell['page']} (zero-based)"
            lines.append(f"- {cell['case']}, {page}, {cell['content']}, {cell['dpi']} dpi: {ratio}")
        lines.append("")
    return "\n".join(lines)
