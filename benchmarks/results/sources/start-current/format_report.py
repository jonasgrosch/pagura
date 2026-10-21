#!/usr/bin/env python3
"""Present a completed independent audit; never calculate statistics or admission."""
import argparse
import hashlib
import json
import math
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def text(value):
    return str(value).replace("|", "\\|").replace("\n", " ").replace("\r", " ")


def number(value):
    require(type(value) in (int, float) and math.isfinite(value) and value > 0,
            "missing/nonpositive/nonfinite audited metric")
    return value


def values(metric, statistics, scale):
    if metric["status"] != "descriptive-all-pairs":
        return ["unavailable: " + text(metric["status"])] + ["—"] * (2 * len(statistics) - 1)
    require(metric["pairs"] == 200 and all(metric[s]["count"] == 200 for s in ("baseline", "candidate")),
            "metric does not contain all 200 pairs")
    result = []
    for statistic in statistics:
        result.append(" → ".join(f'{number(metric[s][statistic]) / scale:.4f}' for s in ("baseline", "candidate")))
        ratio = number(metric[statistic + "_ratio"])
        if statistic == "maximum":
            result.append(f"{ratio:.6f}")
        else:
            bounds = metric["paired_bootstrap"]
            lower, upper = (number(bounds[statistic + suffix]) for suffix in ("_lower", "_upper"))
            result.append(f"{ratio:.6f} [{lower:.6f}, {upper:.6f}]")
    return result


def table(cells, metric, statistics, scale, unit):
    headers = ["Cell (suite / case / mode / operation)", "Audited qualification (B / C)"]
    for statistic in statistics:
        label = "max" if statistic == "maximum" else statistic
        headers.extend([f"{label} B → C ({unit})", f"{label} C/B" + ("" if label == "max" else " [lower, upper]")])
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join(["---"] * len(headers)) + " |"]
    for index, cell in enumerate(cells):
        require(cell["cell_index"] == index, "missing/reordered audit cell")
        quality = cell["quality_qualified"]
        require(all(type(quality[s]) is bool for s in ("baseline", "candidate"))
                and cell["equivalent_output_timing_qualified"] is all(quality.values()), "invalid qualification")
        require(cell["selected_latency_field"] == ("total_ns" if cell["operation"] == "cold" else "render_ns"),
                "unexpected selected latency field")
        name = " / ".join(text(cell[key]) for key in ("suite", "case", "mode", "operation"))
        qualified = " / ".join("qualified" if quality[s] else "unqualified" for s in ("baseline", "candidate"))
        label = text(cell["timing_interpretation"]) + " (" + qualified + ")"
        lines.append("| " + " | ".join([name, label, *values(cell["metrics"][metric], statistics, scale)]) + " |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audit", type=Path, required=True)
    parser.add_argument("--audit-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new Markdown file")
    args = parser.parse_args()
    audit_path = args.audit.resolve(strict=True)
    raw = audit_path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    require(digest == args.audit_sha256, "independent audit hash differs")
    audit = json.loads(raw)
    require(audit["schema"] == 1 and audit["experiment"] == "start-current" and audit["phase"] == "confirmation"
            and audit["status"] == "verified-confirmation" and audit["attempted_processes"] == 34440
            and audit["candidate_quality_verified"] is True and audit["admission"] is False and audit["errors"] == [],
            "requires a complete verified confirmation audit")
    cells = audit["cells"]
    require(len(cells) == 84 and len({tuple(c[k] for k in ("suite", "case", "mode", "operation")) for c in cells}) == 84,
            "requires all 84 distinct cells")
    output = [f"Presentation of [the independent audit](<{audit_path}>), SHA256 `{digest}`.",
              "All 84 cells and 34,440 attempts are represented; measured distributions use 200 pairs per cell. "
              "B = session-start baseline; C = current candidate. Warmups and preflight remain in quality qualification.",
              "Latency uses total_ns for cold and render_ns for full/tile/tiled. B → C shows marginal values; C/B shows "
              "the audited ratio of marginal statistics. Brackets reproduce separate one-sided confidence endpoints, not joint intervals.",
              "Latency (milliseconds)", table(cells, "selected_latency", ("median", "p95"), 1_000_000, "ms"),
              "Peak process RSS (MiB; maximum ratios have no confidence bounds)",
              table(cells, "peak_rss_bytes", ("median", "p95", "maximum"), 1 << 20, "MiB"),
              "All stage summaries, signed paired differences, quality details and provenance remain in the linked audit. "
              "Unqualified baseline cells describe correction cost, never equivalent-output speed gains. "
              "This formatter supplies no aggregate, new gates, resampling, admission or superiority claim.",
              "Auditor limitations: " + " ".join(text(item) for item in audit["limitations"])]
    with args.output.open("x", encoding="utf-8") as handle:
        handle.write("\n\n".join(output) + "\n")


if __name__ == "__main__":
    main()
