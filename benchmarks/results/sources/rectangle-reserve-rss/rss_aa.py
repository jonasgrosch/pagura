#!/usr/bin/env python3
"""One fixed, descriptive Darwin RSS A/A investigation; no admission or retry."""
import argparse
import hashlib
import json
import os
import platform
import random
import re
import signal
import statistics
import sys
import time
import uuid
from pathlib import Path

CELLS = (
    ("classic/graphics-state", "exact"),
    ("paint/text-knockout-true", "exact"),
    ("paint/text-knockout-true", "competitive"),
    ("paint/image-direct-gray-quarter-turn", "exact"),
    ("classic/many-rectangles", "competitive"),
    ("classic/many-operands", "exact"),
    ("classic/many-operands", "competitive"),
)
STRATA = tuple((role, case, mode) for role in ("baseline", "candidate") for case, mode in CELLS)
SEED, WARMUPS, PAIRS, DEADLINE_SECONDS = 20261113, 2, 200, 900
PROCESSES = len(STRATA) * (WARMUPS + PAIRS) * 2
MAX_FILE, MAX_RETAINED, LOG_BUDGET = 64 << 20, 128 << 20, 256 << 10
MAX_EVENT, MAX_JOURNAL = 1 << 20, 128 << 20
RSS_LINE = re.compile(r"^[ \t]*(\d+)[ \t]+maximum resident set size[ \t]*$", re.MULTILINE)


class DeadlineExceeded(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def key(role, case, mode):
    return f"{role}:{case}:{mode}:cold"


def schedule():
    rng, result = random.Random(SEED), []
    for phase, count in (("warmup", WARMUPS), ("measured", PAIRS)):
        orders = {}
        for stratum in STRATA:
            orders[stratum] = ["AB"] * (count // 2) + ["BA"] * (count // 2)
            rng.shuffle(orders[stratum])
        for pair in range(count):
            indices = list(range(len(STRATA)))
            rng.shuffle(indices)
            for index in indices:
                role, case, mode = stratum = STRATA[index]
                result.append({"phase": phase, "pair": pair, "binary_role": role,
                               "case": case, "mode": mode, "operation": "cold",
                               "order": orders[stratum][pair]})
    return result


def rss_record(diagnostic, process):
    raw = diagnostic.encode("ascii", errors="strict")
    require(len(raw) < LOG_BUDGET, "combined process log may be truncated")
    values = RSS_LINE.findall(diagnostic)
    require(len(values) == 1 and re.findall(r"(\d+)\s+maximum resident set size", diagnostic) == values,
            "expected exactly one complete Darwin maximum-RSS field")
    peak = int(values[0])
    require(0 < peak < 1 << 64 and type(process.get("peak_rss_bytes")) is int,
            "missing or invalid Darwin RSS bytes")
    require(peak == process["peak_rss_bytes"] and process.get("rss_status") == "available",
            "raw Darwin RSS field and collector disagree")
    return raw, peak


def distribution(values):
    if not values:
        return {"count": 0}
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive") if len(values) > 1 else [values[0]] * 3
    return {"count": len(values), "values": values, "minimum": min(values), "q1": q1,
            "median": statistics.median(values), "q3": q3, "iqr": q3 - q1, "maximum": max(values)}


def summarize(records):
    result = {}
    for role, case, mode in STRATA:
        pairs = [r for r in records if (r["binary_role"], r["case"], r["mode"]) == (role, case, mode)
                 and r["phase"] == "measured" and all(r.get(s, {}).get("verified") is True for s in "AB")]
        a, b = ([r[s]["rss_bytes"] for r in pairs] for s in "AB")
        item = {"complete_measured_pairs": len(pairs),
                "rss_bytes_by_side": {"A": distribution(a), "B": distribution(b)},
                "paired_B_minus_A_bytes": distribution([y - x for x, y in zip(a, b)]),
                "rss_bytes_by_position": {"first": distribution([r[r["order"][0]]["rss_bytes"] for r in pairs]),
                                          "second": distribution([r[r["order"][1]]["rss_bytes"] for r in pairs])},
                "paired_second_minus_first_bytes": distribution([
                    r[r["order"][1]]["rss_bytes"] - r[r["order"][0]]["rss_bytes"] for r in pairs]),
                "B_minus_A_bytes_by_order": {order: distribution([
                    r["B"]["rss_bytes"] - r["A"]["rss_bytes"] for r in pairs if r["order"] == order])
                    for order in ("AB", "BA")}}
        if pairs:
            item["maximum_ratios"] = {"B_over_A": max(b) / max(a), "A_over_B": max(a) / max(b)}
        result[key(role, case, mode)] = item
    return result


class Journal:
    """Flushed append-only evidence; flush is not a power-loss durability claim."""

    def __init__(self, path):
        self.stream = path.open("xb")
        self.bytes = 0
        self.events = 0

    def append(self, kind, **fields):
        event = {"event": self.events, "type": kind, "monotonic_ns": time.monotonic_ns(), **fields}
        raw = (json.dumps(event, separators=(",", ":"), allow_nan=False) + "\n").encode()
        require(len(raw) <= MAX_EVENT and self.bytes + len(raw) <= MAX_JOURNAL, "journal budget exceeded")
        self.stream.write(raw)
        self.stream.flush()
        self.events += 1
        self.bytes += len(raw)

    def close(self):
        self.stream.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--bindings", type=Path, required=True)
    parser.add_argument("--bindings-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True, help="new directory; never resumed")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("requires Darwin /usr/bin/time -l byte units")
    if not re.fullmatch(r"[0-9a-f]{64}", args.bindings_sha256):
        parser.error("--bindings-sha256 must be an independent SHA-256")
    root, binding_path, directory = (p.resolve() for p in (args.root, args.bindings, args.output))
    directory.mkdir(parents=True, exist_ok=False)
    started, started_unix = time.monotonic(), time.time_ns()
    deadline = started + DEADLINE_SECONDS
    journal = Journal(directory / "events.jsonl")
    report = {"schema": 1, "diagnostic": "rectangle-reserve-darwin-rss-aa", "status": "running",
              "admission": False, "thresholds_changed": False, "records": [],
              "started_unix_ns": started_unix, "deadline_seconds": DEADLINE_SECONDS,
              "producer_argv": list(sys.orig_argv),
              "scope": "seven fixed cold cells, two historical binary strata, same binary for A/B in each stratum",
              "statistics": {"kind": "descriptive only", "quartiles": "inclusive interpolation",
                             "rss_units": "bytes", "maximum_ratios": "ratio of per-label sampled maxima"},
              "limitations": ["No calibrated inference, latency gate, new acceptance test, retry, resume or outlier removal.",
                  "Prior pilot and confirmation failures remain unchanged; A/A variation cannot erase them.",
                  "RSS is the fresh child OS high-water across the whole process, not a periodic sampler.",
                  "Returned combined stdout/stderr must roundtrip as ASCII; no separate stderr claim.",
                  "Canceled-child temporary logs may be unavailable from run_process; attempt identity and outer cancellation remain.",
                  "Deadline stops launches and bounds child work; owned-process cleanup must still finish.",
                  "Loaded source/runtime pins identify this diagnostic, not a rebuild of historical binaries.",
                  "Journal flush is not an fsync or power-loss durability guarantee."],
              "runner": {"run_id": os.environ.get("PAGURA_RUN_ID"), "receipt": os.environ.get("PAGURA_RUN_RECEIPT")}}
    stored, previous, helpers, manifest = 0, {}, None, None

    def checkpoint():
        report["stored_bytes"] = stored
        temporary = directory / "report.pending"
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        temporary.replace(directory / "report.json")

    def check_deadline():
        if time.monotonic() >= deadline:
            raise DeadlineExceeded("fixed 900-second diagnostic deadline reached")

    def retain(relative, raw):
        nonlocal stored
        path = directory / relative
        if path.exists():
            require(helpers.read_bounded_regular(path, MAX_FILE) == raw, "retained evidence changed: " + relative)
            return
        require(len(raw) <= MAX_FILE and stored + len(raw) <= MAX_RETAINED, "retention budget exceeded")
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as handle:
            handle.write(raw)
        path.chmod(0o400)
        stored += len(raw)

    def pinned(path, pin):
        raw = helpers.read_bounded_regular(path, MAX_FILE)
        require(sha(raw) == pin, "frozen evidence changed: " + str(path))
        return raw

    checkpoint()
    try:
        sys.dont_write_bytecode = True
        sys.path[:0] = [str(root / "tools"), str(root / "benchmarks")]
        from benchkit import artifacts as helpers
        from benchkit.config import ROOT
        from benchkit.images import normalize
        from benchkit.measurement import MeasurementCancelled, rss_wrapper, run_process
        import color_reuse_fixtures as fixtures
        require(Path(ROOT).resolve() == root and rss_wrapper() == ["/usr/bin/time", "-l"],
                "wrong helper root or RSS collector")

        def cancel(signum, _frame):
            raise MeasurementCancelled(signum)

        previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
        for signum in previous:
            signal.signal(signum, cancel)
        pointer = report["runner"]
        require(isinstance(pointer["run_id"], str) and str(uuid.UUID(pointer["run_id"])) == pointer["run_id"]
                and isinstance(pointer["receipt"], str) and Path(pointer["receipt"]).is_absolute(),
                "missing canonical runner identity")
        start_raw = helpers.read_bounded_regular(Path(pointer["receipt"]), MAX_FILE)
        start_receipt = json.loads(start_raw)
        require(type(start_receipt.get("schema")) is int and start_receipt["schema"] == 1
                and start_receipt.get("status") == "running" and start_receipt.get("kind") == "benchmark"
                and start_receipt.get("run_id") == pointer["run_id"]
                and start_receipt.get("source_root") == start_receipt.get("cwd") == str(root)
                and isinstance(start_receipt.get("source_before"), dict), "runner start record differs")
        retain("runner-start.json", start_raw)
        report["runner_source_before"] = start_receipt["source_before"]
        journal.append("run-start", runner=pointer, source_before=start_receipt["source_before"],
                       producer_argv=report["producer_argv"], started_unix_ns=started_unix)
        binding_raw = pinned(binding_path, args.bindings_sha256)
        binding = json.loads(binding_raw)
        identities = binding["identities"]
        require(binding.get("experiment") == "rectangle-reserve" and set(identities) == {"baseline", "candidate"}
                and Path(identities["candidate"]["root"]).resolve() == root, "wrong frozen experiment identities")
        binaries = {role: Path(identity["binary"]).resolve() for role, identity in identities.items()}
        for role, binary in binaries.items():
            require(str(binary) == identities[role]["binary"]
                    and helpers.hash_regular_file(binary) == identities[role]["binary_sha256"], "binary identity mismatch")
        retain("bindings.json", binding_raw)
        build_info = binding["build_report"]
        build_raw = pinned(Path(build_info["path"]), build_info["sha256"])
        build = json.loads(build_raw)
        require(build.get("status") == "built-both-arms" and build.get("identities") == identities,
                "historical build identities differ")
        for role, identity in identities.items():
            source = {k: identity[k] for k in ("root", "commit", "source_sha256")}
            require(build["builds"][role]["source_before"] == build["builds"][role]["source_after"] == source
                    and type(build["builds"][role]["process"].get("exit_code")) is int
                    and build["builds"][role]["process"]["exit_code"] == 0, "historical build/source mismatch")
        evidence = [item for item in binding["completed_evidence"]
                    if item["artifacts"].get(build_info["path"]) == build_info["sha256"]]
        require(len(evidence) == 1, "ambiguous historical build receipt")
        build_evidence = evidence[0]
        build_receipt_raw = pinned(Path(build_evidence["receipt"]["path"]), build_evidence["receipt"]["sha256"])
        build_receipt = json.loads(build_receipt_raw)
        require(build_receipt.get("status") == "completed" and build_receipt.get("source_unchanged") is True
                and build_receipt.get("errors") == [] and build_receipt.get("artifact_errors") == {}
                and all(type(build_receipt.get(k)) is int and build_receipt[k] == 0 for k in ("exit_code", "wrapper_exit_code"))
                and build_receipt["artifacts"].get(build_info["path"]) == build_info["sha256"], "invalid historical build receipt")
        retain("historical-build.json", build_raw)
        retain("historical-build-receipt.json", build_receipt_raw)
        retain("historical-build-guard.json", pinned(Path(build_evidence["guard"]["path"]), build_evidence["guard"]["sha256"]))
        for relative in ("benchmarks/color_reuse_fixtures.py", "benchmarks/paired_render.py",
                         "benchmarks/results/sources/analytic19-conversion/manifest.json",
                         "benchmarks/results/sources/analytic19-conversion/references.json",
                         "crates/pagura-render/examples/scene_index_profile.rs", "tools/benchkit/measurement.py"):
            pinned(root / relative, binding["bound_files_sha256"][relative])
        retain("references-manifest.json", pinned(fixtures.MANIFEST, fixtures.MANIFEST_SHA256))
        retain("references.json", pinned(fixtures.REFERENCES, fixtures.REFERENCES_SHA256))
        inputs, expected = {}, {}
        for case in dict.fromkeys(case for case, _ in CELLS):
            check_deadline()
            pdf, oracle = fixtures.fixture(case)
            retain("inputs/" + case + ".pdf", pdf)
            retain("oracles/" + case + ".ppm", oracle)
            expected[case] = normalize(oracle)
            inputs[case] = {"pdf_sha256": sha(pdf), "oracle_sha256": sha(oracle),
                            "width": expected[case][0], "height": expected[case][1]}
        paths = {Path(__file__).resolve(), Path(sys.executable).resolve(), Path("/usr/bin/time"),
                 Path("/bin/ps"), binding_path, fixtures.MANIFEST, fixtures.REFERENCES,
                 root / "crates/pagura-render/examples/scene_index_profile.rs"}
        for module in tuple(sys.modules.values()):
            path = getattr(module, "__file__", None)
            if path and Path(path).is_file():
                paths.add(Path(path).resolve())
        manifest = {str(path): helpers.hash_regular_file(path) for path in sorted(paths)}
        plan = {"schema": 1, "seed": SEED, "warmup_pairs_per_stratum": WARMUPS,
                "measured_pairs_per_stratum": PAIRS, "processes": PROCESSES,
                "deadline_seconds": DEADLINE_SECONDS, "operation": "cold", "dpi": 72,
                "cells": [{"case": c, "mode": m} for c, m in CELLS], "binary_roles": ["baseline", "candidate"],
                "same_absolute_binary_for_both_labels_within_stratum": True,
                "identities": identities, "inputs": inputs, "bindings_sha256": sha(binding_raw),
                "bound_files_sha256": manifest, "schedule": schedule(),
                "runner_start_sha256": sha(start_raw), "runner_source_before": start_receipt["source_before"],
                "runtime": {"python": sys.version, "executable": str(Path(sys.executable).resolve()),
                    "platform": platform.platform(), "machine": platform.machine(), "time_wrapper": rss_wrapper(),
                    "environment": {k: os.environ.get(k) for k in ("LANG", "LC_ALL", "MallocNanoZone",
                        "MallocScribble", "MallocGuardEdges", "MallocStackLogging", "MallocStackLoggingNoCompact",
                        "MALLOC_CONF", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH")}},
                "oracle": "unchanged externally pinned analytic19 PDF and RGB references; exact RGB and opaque alpha",
                "rss": {"source": "/usr/bin/time -l", "units": "bytes", "scope": "one fresh renderer child high-water"},
                "journal": {"path": "events.jsonl", "maximum_event_bytes": MAX_EVENT, "maximum_bytes": MAX_JOURNAL}}
        plan_raw = (json.dumps(plan, indent=2, allow_nan=False) + "\n").encode()
        retain("plan.json", plan_raw)
        report.update(plan_sha256=sha(plan_raw), identities=identities)
        for sequence, planned in enumerate(plan["schedule"]):
            check_deadline()
            pair = {**planned, "sequence": sequence, "started_monotonic_ns": time.monotonic_ns()}
            report["records"].append(pair)
            journal.append("pair-start", pair_sequence=sequence, scheduled=planned,
                           started_monotonic_ns=pair["started_monotonic_ns"])
            case, mode, role = pair["case"], pair["mode"], pair["binary_role"]
            binary, pdf_path = binaries[role], directory / "inputs" / (case + ".pdf")
            for label in pair["order"]:
                check_deadline()
                require(helpers.hash_regular_file(binary) == identities[role]["binary_sha256"], "binary changed before launch")
                require(sha(helpers.read_bounded_regular(pdf_path, MAX_FILE)) == inputs[case]["pdf_sha256"], "input changed")
                output = directory / "pending.rgba"
                require(not output.exists(), "unexpected pending output")
                command = [str(binary), str(pdf_path), mode, "cold", str(output), "72"]
                record = {"command": command, "started_monotonic_ns": time.monotonic_ns(), "verified": False}
                pair[label] = record
                journal.append("arm-start", pair_sequence=sequence, label=label,
                               command=command, started_monotonic_ns=record["started_monotonic_ns"])
                try:
                    policy = {"measure_rss": True, "timeout_seconds": min(120, deadline - time.monotonic())}
                    if policy["timeout_seconds"] <= 0:
                        raise DeadlineExceeded("deadline before child launch")
                    process, diagnostic = run_process(command, policy, log_budget=LOG_BUDGET)
                    record["process"] = process
                    log_name = f"logs/{sequence:04d}-{label}.stdout-stderr.txt"
                    log_raw = diagnostic.encode("utf-8")
                    retain(log_name, log_raw)
                    record.update(log=log_name, log_sha256=sha(log_raw), log_bytes=len(log_raw))
                    check_deadline()
                    raw_ascii, peak = rss_record(diagnostic, process)
                    require(raw_ascii == log_raw, "returned log did not roundtrip as ASCII")
                    record["rss_bytes"] = peak
                    require(process.get("exit_code") == 0 and process.get("status") != "timeout", "renderer failed")
                    raw = helpers.read_bounded_regular(output, MAX_FILE)
                    blob = "blobs/" + sha(raw) + ".rgba"
                    retain(blob, raw)
                    record.update(blob=blob, output_sha256=sha(raw))
                    width, height, _ = expected[case]
                    header = f"RGBA8 {width} {height} premultiplied\n".encode()
                    require(raw.startswith(header) and len(raw) == len(header) + width * height * 4
                            and raw[len(header) + 3::4] == b"\xff" * (width * height)
                            and normalize(raw) == expected[case], "independent RGB/alpha mismatch")
                    stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
                    require(len(stages) == 1 and isinstance(stages[0], dict), "missing unique stage record")
                    stage = stages[0]
                    require(stage.get("schema") == 1 and stage.get("accuracy") == mode
                            and stage.get("operation") == "cold" and stage.get("dpi") == 72
                            and stage.get("width") == width and stage.get("height") == height
                            and all(type(stage.get(k)) is int and stage[k] > 0 for k in
                                    ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
                            and stage["metric_ns"] == stage["total_ns"]
                            and stage["total_ns"] >= stage["open_ns"] + stage["compile_ns"] + stage["render_ns"],
                            "stage identity/counters differ")
                    record.update(stages=stage, verified=True)
                    output.unlink()
                    check_deadline()
                except BaseException as error:
                    record["error"] = f"{type(error).__name__}: {error}"
                    if isinstance(error, (KeyboardInterrupt, SystemExit)):
                        record["interrupted"] = True
                    if output.exists() and "blob" not in record:
                        try:
                            raw = helpers.read_bounded_regular(output, MAX_FILE)
                            blob = "blobs/" + sha(raw) + ".rgba"
                            retain(blob, raw)
                            record.update(blob=blob, output_sha256=sha(raw))
                        except (OSError, ValueError) as artifact_error:
                            record["artifact_error"] = str(artifact_error)
                    raise
                finally:
                    record["finished_monotonic_ns"] = time.monotonic_ns()
                    journal.append("arm-result", pair_sequence=sequence, label=label, record=record)
            require(pair["A"]["output_sha256"] == pair["B"]["output_sha256"], "same-binary output bytes differ")
            pair["finished_monotonic_ns"] = time.monotonic_ns()
            journal.append("pair-finished", pair_sequence=sequence, finished_monotonic_ns=pair["finished_monotonic_ns"])
            if (sequence + 1) % len(STRATA) == 0:
                print(f"completed RSS round {(sequence + 1) // len(STRATA)}/{WARMUPS + PAIRS}", flush=True)
        for role, binary in binaries.items():
            require(helpers.hash_regular_file(binary) == identities[role]["binary_sha256"], "binary changed during diagnostic")
        require(all(helpers.hash_regular_file(Path(p)) == pin for p, pin in manifest.items()), "bound code/runtime changed")
        report["summaries"] = summarize(report["records"])
        verified = sum(r.get(s, {}).get("verified") is True for r in report["records"] for s in "AB")
        require(verified == PROCESSES and all(s["complete_measured_pairs"] == PAIRS for s in report["summaries"].values()),
                "fixed schedule incomplete")
        check_deadline()
        report.update(status="completed-diagnostic", verified_processes=verified)
        return_code = 0
    except BaseException as error:
        report["status"] = ("deadline-exceeded" if isinstance(error, DeadlineExceeded) else
                            "cancelled" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "failed")
        report["error"] = f"{type(error).__name__}: {error}"
        report["summaries_of_complete_pairs_only"] = summarize(report["records"])
        return_code = (128 + getattr(error, "signum", signal.SIGINT) if report["status"] == "cancelled"
                       else 124 if report["status"] == "deadline-exceeded" else 1)
    finally:
        # Signals remain handled until terminal evidence is recorded. A second
        # interruption may leave an incomplete journal; the outer receipt rejects it.
        try:
            report["finished_unix_ns"] = time.time_ns()
            report["duration_seconds"] = time.monotonic() - started
            if report["status"] == "completed-diagnostic" and report["duration_seconds"] > DEADLINE_SECONDS:
                report.update(status="deadline-exceeded", error="deadline crossed during finalization")
                return_code = 124
            try:
                journal.append("run-finished", status=report["status"], finished_unix_ns=report["finished_unix_ns"],
                               duration_seconds=report["duration_seconds"], error=report.get("error"))
            except (OSError, ValueError) as journal_error:
                report.update(status="failed", journal_error=str(journal_error))
                return_code = 1
            finally:
                journal.close()
            try:
                journal_sha = helpers.hash_regular_file(directory / "events.jsonl") if helpers is not None else None
            except (OSError, ValueError) as journal_error:
                report.update(status="failed", journal_error=str(journal_error))
                journal_sha, return_code = None, 1
            report["journal"] = {"path": "events.jsonl", "sha256": journal_sha,
                                 "bytes": journal.bytes, "events": journal.events}
            checkpoint()
        finally:
            journal.close()
            for signum, handler in previous.items():
                signal.signal(signum, handler)
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
