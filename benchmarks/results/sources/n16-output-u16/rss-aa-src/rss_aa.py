#!/usr/bin/env python3
"""Fixed Darwin same-binary RSS diagnostic; never performance admission."""
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
from pathlib import Path

CASES = ("outline2-knockout", "default-knockout64", "opaque64")
SEED, WARMUPS, PAIRS, DEADLINE_SECONDS = 20261104, 2, 12, 600
MAX_FILE, MAX_RETAINED, LOG_BUDGET = 64 << 20, 128 << 20, 256 << 10
RSS_LINE = re.compile(r"^[ \t]*(\d+)[ \t]+maximum resident set size[ \t]*$", re.MULTILINE)


class DeadlineExceeded(Exception):
    pass


def schedule():
    """Balanced label order per case/phase; shuffle cells within every round."""
    rng = random.Random(SEED)
    result = []
    for phase, count in (("warmup", WARMUPS), ("measured", PAIRS)):
        orders = {}
        for case in CASES:
            orders[case] = ["AB"] * (count // 2) + ["BA"] * (count // 2)
            rng.shuffle(orders[case])
        for pair in range(count):
            cells = list(CASES)
            rng.shuffle(cells)
            for case in cells:
                result.append({"phase": phase, "pair": pair, "case": case,
                               "order": orders[case][pair]})
    return result


def rss_record(diagnostic, process):
    """Verify the helper's parsed bytes against its complete returned ASCII log."""
    raw = diagnostic.encode("ascii", errors="strict")
    if len(raw) >= LOG_BUDGET:
        raise ValueError("combined process log may be truncated")
    values = RSS_LINE.findall(diagnostic)
    # The existing helper chooses the last unanchored match. One complete,
    # anchored field rules out ambiguous or embedded duplicate RSS claims.
    if len(values) != 1 or re.findall(r"(\d+)\s+maximum resident set size", diagnostic) != values:
        raise ValueError("expected exactly one complete Darwin maximum-RSS field")
    peak = int(values[0])
    if not 0 < peak <= (1 << 64) - 1 or type(process.get("peak_rss_bytes")) is not int:
        raise ValueError("missing or nonpositive Darwin RSS bytes")
    if peak != process["peak_rss_bytes"] or process.get("rss_status") != "available":
        raise ValueError("raw Darwin RSS field and collector result disagree")
    return raw, peak


def distribution(values):
    if not values:
        return {"count": 0}
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive") if len(values) > 1 else [values[0]] * 3
    return {"count": len(values), "values": values, "minimum": min(values),
            "q1": q1, "median": statistics.median(values), "q3": q3,
            "iqr": q3 - q1, "maximum": max(values)}


def summarize(records):
    """Descriptive only: no bootstrap, calibrated interval, gate or outlier cut."""
    result = {}
    for case in CASES:
        pairs = [r for r in records if r["case"] == case and r["phase"] == "measured"
                 and all(r.get(s, {}).get("verified") is True for s in "AB")]
        a = [r["A"]["rss_bytes"] for r in pairs]
        b = [r["B"]["rss_bytes"] for r in pairs]
        item = {"complete_measured_pairs": len(pairs),
                "rss_bytes_by_side": {"A": distribution(a), "B": distribution(b)},
                "paired_B_minus_A_bytes": distribution([y - x for x, y in zip(a, b)]),
                "rss_bytes_by_position": {
                    "first": distribution([r[r["order"][0]]["rss_bytes"] for r in pairs]),
                    "second": distribution([r[r["order"][1]]["rss_bytes"] for r in pairs])},
                "paired_second_minus_first_bytes": distribution([
                    r[r["order"][1]]["rss_bytes"] - r[r["order"][0]]["rss_bytes"] for r in pairs]),
                "B_minus_A_bytes_by_order": {order: distribution([
                    r["B"]["rss_bytes"] - r["A"]["rss_bytes"] for r in pairs if r["order"] == order])
                    for order in ("AB", "BA")}}
        if pairs:
            item["maximum_ratios"] = {"B_over_A": max(b) / max(a), "A_over_B": max(a) / max(b)}
        result[case] = item
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True, help="frozen helper checkout")
    parser.add_argument("--bindings", type=Path, required=True, help="frozen candidate bindings JSON")
    parser.add_argument("--binary", type=Path, required=True, help="same absolute binary for A and B")
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory; never resumed")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("this diagnostic fixes /usr/bin/time -l and Darwin byte units")
    root, binary, binding_path = (p.resolve() for p in (args.root, args.binary, args.bindings))
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    deadline = started + DEADLINE_SECONDS
    report = {"schema": 1, "diagnostic": "same-binary-darwin-rss-aa", "status": "running",
              "admission": False, "thresholds_changed": False, "records": [],
              "started_unix_ns": time.time_ns(), "deadline_seconds": DEADLINE_SECONDS,
              "scope": "three fixed Exact/full native fresh-process cases; retained-list render operation",
              "statistics": {"kind": "descriptive only", "quartiles": "inclusive interpolation",
                             "rss_units": "bytes", "maximum_ratios": "ratio of per-side sampled maxima"},
              "limitations": ["descriptive diagnostic, not calibrated confidence or admission",
                  "existing failed performance/RSS gates remain unchanged",
                  "no periodic RSS sampler: OS child high-water via /usr/bin/time -l",
                  "combined stdout/stderr ASCII text returned by the existing helper, not separate stderr",
                  "fixed deadline stops launches and bounds each child; owned-process cleanup must finish",
                  "same local binary and coordinator build bindings, not an independently reproduced build"],
              "runner": {"run_id": os.environ.get("PAGURA_RUN_ID"),
                         "receipt": os.environ.get("PAGURA_RUN_RECEIPT")}}
    stored = 0
    helpers = None
    manifest = None
    previous = {}

    def checkpoint():
        report["stored_bytes"] = stored
        temporary = directory / "report.pending"
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        temporary.replace(directory / "report.json")

    def check_deadline():
        if time.monotonic() >= deadline:
            raise DeadlineExceeded("fixed 600-second diagnostic deadline reached")

    def retain(relative, data):
        nonlocal stored
        path = directory / relative
        if path.exists():
            if helpers.read_bounded_regular(path, MAX_FILE) != data:
                raise ValueError("retained evidence differs at " + relative)
            return
        if len(data) > MAX_FILE or stored + len(data) > MAX_RETAINED:
            raise ValueError("fixed evidence retention budget exceeded")
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as output:
            output.write(data)
        path.chmod(0o400)
        stored += len(data)

    def digest(data):
        return hashlib.sha256(data).hexdigest()

    try:
        sys.dont_write_bytecode = True
        sys.path[:0] = [str(root / "tools"), str(root / "benchmarks")]
        from benchkit import artifacts as helpers
        from benchkit.config import ROOT
        from benchkit.images import normalize
        from benchkit.measurement import MeasurementCancelled, rss_wrapper, run_process
        import scene_index
        import glyph_memo_fixtures
        if Path(ROOT).resolve() != root or rss_wrapper() != ["/usr/bin/time", "-l"]:
            raise ValueError("imported helper root or RSS wrapper differs")

        def cancel(signum, _frame):
            raise MeasurementCancelled(signum)

        previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
        for signum in previous:
            signal.signal(signum, cancel)
        binding_raw = helpers.read_bounded_regular(binding_path, MAX_FILE)
        binding = json.loads(binding_raw)
        identity = binding["identities"]["candidate"]
        binary_sha = helpers.hash_regular_file(binary)
        if (Path(identity["binary"]).resolve() != binary or identity["binary_sha256"] != binary_sha
                or Path(identity["root"]).resolve() != root):
            raise ValueError("binary/root differ from the frozen candidate bindings")
        retain("bindings.json", binding_raw)
        inputs, expected = {}, {}
        for case in CASES:
            check_deadline()
            pdf, oracle = (glyph_memo_fixtures.fixture(case) if case == "outline2-knockout"
                           else scene_index.fixture(case))
            retain("inputs/" + case + ".pdf", pdf)
            retain("oracles/" + case + ".ppm", oracle)
            expected[case] = normalize(oracle)
            inputs[case] = {"pdf_sha256": digest(pdf), "oracle_sha256": digest(oracle),
                            "width": expected[case][0], "height": expected[case][1]}
        # Bind all loaded file-backed helpers and Python runtime modules, plus
        # native collector/runtime executables and the native driver source.
        paths = {Path(__file__).resolve(), Path(sys.executable).resolve(),
                 Path("/usr/bin/time"), Path("/bin/ps"), binding_path,
                 root / "crates/pagura-render/examples/scene_index_profile.rs"}
        for module in tuple(sys.modules.values()):
            name = getattr(module, "__file__", None)
            if name and Path(name).is_file():
                paths.add(Path(name).resolve())
        manifest = {str(p): helpers.hash_regular_file(p) for p in sorted(paths)}
        plan = {"schema": 1, "seed": SEED, "warmup_pairs_per_case": WARMUPS,
                "measured_pairs_per_case": PAIRS, "processes": 84, "deadline_seconds": DEADLINE_SECONDS,
                "mode": "exact", "operation": "full", "dpi": 72,
                "binary": str(binary), "binary_sha256": binary_sha,
                "same_absolute_binary_for_both_labels": True, "inputs": inputs,
                "bindings_sha256": digest(binding_raw), "bound_files_sha256": manifest,
                "schedule": schedule(), "runtime": {"python": sys.version,
                    "executable": str(Path(sys.executable).resolve()), "platform": platform.platform(),
                    "machine": platform.machine(), "time_wrapper": rss_wrapper(),
                    "environment": {k: os.environ.get(k) for k in (
                        "LANG", "LC_ALL", "MallocNanoZone", "MallocScribble", "MallocGuardEdges",
                        "MallocStackLogging", "MallocStackLoggingNoCompact", "MALLOC_CONF",
                        "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH")}},
                "oracle": "independent integer rectangle partitions: opaque black or quarter-alpha black (191) over white",
                "rss": {"source": "/usr/bin/time -l", "units": "bytes", "scope": "one renderer child high-water"}}
        plan_raw = (json.dumps(plan, indent=2, allow_nan=False) + "\n").encode()
        retain("plan.json", plan_raw)
        report["plan_sha256"] = digest(plan_raw)
        report["binary_sha256"] = binary_sha
        checkpoint()
        for sequence, scheduled in enumerate(plan["schedule"]):
            check_deadline()
            pair = {**scheduled, "sequence": sequence, "started_monotonic_ns": time.monotonic_ns()}
            report["records"].append(pair)
            case = pair["case"]
            for side in pair["order"]:
                check_deadline()
                pdf_path = directory / "inputs" / (case + ".pdf")
                if helpers.hash_regular_file(binary) != binary_sha:
                    raise ValueError("candidate binary changed before launch")
                if digest(helpers.read_bounded_regular(pdf_path, MAX_FILE)) != inputs[case]["pdf_sha256"]:
                    raise ValueError("retained input changed before launch")
                output = directory / "pending.rgba"
                if output.exists():
                    raise ValueError("unexpected output from an earlier invocation")
                command = [str(binary), str(pdf_path), "exact", "full", str(output), "72"]
                record = {"command": command, "started_monotonic_ns": time.monotonic_ns(), "verified": False}
                pair[side] = record
                checkpoint()
                policy = {"measure_rss": True, "timeout_seconds": min(120, deadline - time.monotonic())}
                if policy["timeout_seconds"] <= 0:
                    raise DeadlineExceeded("deadline before launch")
                process, diagnostic = run_process(command, policy, log_budget=LOG_BUDGET)
                record["process"] = process
                record["finished_monotonic_ns"] = time.monotonic_ns()
                log_name = f"logs/{sequence:03d}-{side}.stdout-stderr.txt"
                # Persist all returned text even on a child failure. UTF-8 here
                # preserves the returned string; ASCII roundtrip is mandatory
                # for a verified record and invalid decoding blocks completion.
                log_raw = diagnostic.encode("utf-8")
                retain(log_name, log_raw)
                record.update(log=log_name, log_sha256=digest(log_raw), log_bytes=len(log_raw))
                check_deadline()
                raw_ascii, peak = rss_record(diagnostic, process)
                if raw_ascii != log_raw:
                    raise ValueError("returned process log did not roundtrip as ASCII")
                record["rss_bytes"] = peak
                if process.get("exit_code") != 0 or process.get("status") == "timeout":
                    raise ValueError("renderer invocation failed; no further samples")
                raw = helpers.read_bounded_regular(output, MAX_FILE)
                blob = "blobs/" + digest(raw) + ".rgba"
                retain(blob, raw)
                actual = normalize(raw)
                width, height, _ = expected[case]
                header = f"RGBA8 {width} {height} premultiplied\n".encode()
                if (not raw.startswith(header) or len(raw) != len(header) + width * height * 4
                        or raw[len(header) + 3::4] != b"\xff" * (width * height)
                        or actual != expected[case]):
                    raise ValueError("independent rectangle/alpha oracle mismatch")
                stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith("{")]
                if (len(stages) != 1 or not isinstance(stages[0], dict)
                        or stages[0].get("schema") != 1 or stages[0].get("accuracy") != "exact"
                        or stages[0].get("operation") != "full" or stages[0].get("dpi") != 72
                        or stages[0].get("width") != width or stages[0].get("height") != height):
                    raise ValueError("driver stage identity mismatch")
                stage = stages[0]
                if (any(type(stage.get(k)) is not int or stage[k] <= 0 for k in
                        ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns"))
                        or stage["metric_ns"] != stage["render_ns"]
                        or stage["total_ns"] < stage["open_ns"] + stage["compile_ns"] + stage["render_ns"]):
                    raise ValueError("invalid stage counters")
                record.update(blob=blob, output_sha256=digest(raw), stages=stage, verified=True)
                output.unlink()
                checkpoint()
                check_deadline()
            if pair["A"]["output_sha256"] != pair["B"]["output_sha256"]:
                raise ValueError("A/A output bytes differ")
            pair["finished_monotonic_ns"] = time.monotonic_ns()
            checkpoint()
            print(f"completed diagnostic pair {sequence + 1}/42: {case} {pair['phase']}", flush=True)
        if helpers.hash_regular_file(binary) != binary_sha:
            raise ValueError("candidate binary changed during diagnostic")
        changed = [p for p, pin in manifest.items() if helpers.hash_regular_file(Path(p)) != pin]
        if changed:
            raise ValueError("bound code/runtime changed: " + ", ".join(changed))
        report["summaries"] = summarize(report["records"])
        verified = sum(r.get(side, {}).get("verified") is True for r in report["records"] for side in "AB")
        if verified != 84 or any(r["complete_measured_pairs"] != PAIRS for r in report["summaries"].values()):
            raise ValueError("fixed sample schedule did not complete exactly")
        check_deadline()
        report["status"] = "completed-diagnostic"
        report["verified_processes"] = verified
        return_code = 0
    except BaseException as error:
        report["status"] = ("deadline-exceeded" if isinstance(error, DeadlineExceeded) else
                            "cancelled" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "failed")
        report["error"] = f"{type(error).__name__}: {error}"
        report["summaries_of_complete_pairs_only"] = summarize(report["records"])
        return_code = (128 + getattr(error, "signum", signal.SIGINT) if report["status"] == "cancelled"
                       else 124 if report["status"] == "deadline-exceeded" else 1)
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        report["finished_unix_ns"] = time.time_ns()
        report["duration_seconds"] = time.monotonic() - started
        checkpoint()
        (directory / "report.json").chmod(0o400)
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
