#!/usr/bin/env python3
"""Independent retained-evidence audit of the one frozen native F32 cost trial.

No producer imports, rendering, timing, resampling, or release admission.
The coordinator supplies a separately pinned final evidence manifest.
"""
import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import random
import re
import stat
import statistics
import subprocess

LIMIT = 64 * 1024 * 1024
CASES = (
    "classic/blank", "classic/rectangle", "classic/graphics-state",
    "classic/negative-rectangle", "classic/inherited-box", "classic/clipped-to-page",
    "classic/many-rectangles", "classic/content-array-split", "classic/many-operands",
    "paint/text-heavy-page", "paint/text-mode-4", "paint/type3-d0-colour",
    "paint/text-knockout-true", "paint/text-knockout-false", "paint/image-soft-mask",
    "paint/image-direct-binary-halves", "paint/image-direct-gray-quarter-turn",
    "paint/image-direct-rgb-fractional", "paint/image-direct-bilinear",
)
CELLS = tuple((case, mode, "cold") for case in CASES for mode in ("exact", "competitive"))
SIDES = ("baseline", "candidate")
KNOWN = ("paint/text-knockout-false", "competitive", "cold")
PLAN = {"schema": 1, "cases": list(CASES), "cells": [list(c) for c in CELLS],
        "metric": "total_ns", "seed": 20261110, "warmup_pairs": 5, "measured_pairs": 31,
        "expected_preflight_outputs": 76, "expected_cost_outputs": 2736,
        "measured_order_per_cell": {"baseline_first": 16, "candidate_first": 15},
        "warmup_order_per_cell": {"baseline_first": 3, "candidate_first": 2}}
MANIFEST = "278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3"
REFERENCES = "924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf"
SHA = lambda raw: hashlib.sha256(raw).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path, limit=LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        require(stat.S_ISREG(os.fstat(fd).st_mode), "nonregular evidence: " + str(path))
        with os.fdopen(fd, "rb", closefd=False) as stream:
            raw = stream.read(limit + 1)
    finally:
        os.close(fd)
    require(len(raw) <= limit, "oversized evidence: " + str(path))
    return raw


def bound(item):
    require(set(item) == {"path", "sha256"}, "expected path/sha256 pin")
    require(Path(item["path"]).is_absolute(), "pins require absolute paths")
    raw = read(item["path"], 256 * 1024 * 1024)
    require(SHA(raw) == item["sha256"], "pin mismatch: " + item["path"])
    return raw


def within(root, relative):
    require(type(relative) is str and not Path(relative).is_absolute()
            and ".." not in Path(relative).parts, "unsafe relative artifact")
    path = (root / relative).resolve()
    require(path.is_relative_to(root.resolve()), "artifact escapes root")
    return path


def source(root, runner=False):
    paths = set()
    for directory in ("crates", "tools", "benchmarks/adapters"):
        paths.update(p for p in (root / directory).rglob("*") if p.is_file()
                     and "target" not in p.parts and "__pycache__" not in p.parts)
    paths.update((root / "corpus/manifests").glob("*.json"))
    names = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "benchmarks/policy.json"]
    if runner:
        names += ["benchmarks/release-matrix.json", "benchmarks/requirements.txt",
                  "benchmarks/node-package.json", "benchmarks/node-package-lock.json"]
    paths.update(root / name for name in names if not runner or (root / name).exists())
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0" + read(path))
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    return {"root": str(root), "commit": commit, "source_sha256": digest.hexdigest()}


def ppm(raw):
    match = re.match(rb"P6\n([1-9][0-9]*) ([1-9][0-9]*)\n255\n", raw)
    require(match is not None, "expected frozen RGB8 P6 oracle")
    w, h = map(int, match.groups())
    pixels = raw[match.end():]
    require(len(pixels) == w * h * 3, "oracle length mismatch")
    return w, h, pixels


def quality(raw, expected):
    w, h, target = expected
    header = f"RGBA8 {w} {h} premultiplied\n".encode()
    require(raw.startswith(header), "actual format/dimensions mismatch")
    body = raw[len(header):]
    require(len(body) == w * h * 4, "actual RGBA length mismatch")
    require(body[3::4] == b"\xff" * (w * h), "nonopaque actual alpha")
    rgb = bytes(value for i, value in enumerate(body) if i % 4 != 3)
    differences = [abs(a - b) for a, b in zip(rgb, target)]
    return ({"dimensions_equal": True, "opaque_alpha_equal": True, "rgb_equal": rgb == target,
             "max_channel_error": max(differences, default=0),
             "different_channels": sum(d != 0 for d in differences)},
            sorted({(a, b) for a, b in zip(rgb, target) if a != b}))


def schedule():
    rng, result = random.Random(20261110), []
    for warm, count in ((True, 5), (False, 31)):
        phase = []
        for cell in CELLS:
            orders = [SIDES] * ((count + 1) // 2) + [SIDES[::-1]] * (count // 2)
            rng.shuffle(orders)
            phase.extend([list(cell), warm, i, list(order)] for i, order in enumerate(orders))
        rng.shuffle(phase)
        result.extend(phase)
    return result


def producer_command(pin, root, phase, directory, pins):
    command = pin["producer_argv"]
    require(command[:3] == ["python3", "-B", "benchmarks/results/sources/f32-preservation/cost.py"]
            and len(command[3:]) % 2 == 0, "not the frozen direct producer command")
    pairs = list(zip(command[3::2], command[4::2]))
    require(len({key for key, _ in pairs}) == len(pairs), "duplicate producer option")
    options = dict(pairs)
    expected = {"--bindings": pins["bindings"]["path"], "--output": str(directory), "--phase": phase}
    if phase == "cost":
        expected.update({"--preflight": pins["preflight"]["report"]["path"],
                         "--preflight-sha256": pins["preflight"]["report"]["sha256"]})
    require(options == expected, "producer options differ from independent phase bindings")
    return command


def final_coordination(pin, report, report_pin, root, runner_identity, command):
    receipt, guard = (json.loads(bound(pin[key])) for key in ("receipt", "guard"))
    require(receipt["schema"] == 1 and receipt["status"] == "completed"
            and receipt["kind"] == "benchmark" and receipt["runner"]["system"] == "Darwin", "nonterminal/non-Darwin outer receipt")
    require(receipt["exit_code"] == receipt["wrapper_exit_code"] == 0
            and receipt["cancellation_signal"] is None and receipt["errors"] == []
            and receipt["artifact_errors"] == {} and receipt["source_unchanged"] is True,
            "failed or uncertain runner")
    require(receipt["cwd"] == receipt["source_root"] == str(root), "runner root mismatch")
    require(report["run_id"] == receipt["run_id"] and report["receipt"] == pin["receipt"]["path"],
            "report/runner pointer mismatch")
    require(receipt["environment_overrides"] == {"PAGURA_RUN_ID": receipt["run_id"],
            "PAGURA_RUN_RECEIPT": pin["receipt"]["path"]}, "runner environment mismatch")
    identity = {"commit": runner_identity["commit"], "sha256": runner_identity["source_sha256"]}
    require(receipt["source_before"] == receipt["source_after"] == identity, "outer source mismatch")
    times = [datetime.fromisoformat(receipt[k]) for k in ("started_at", "ended_at", "finalized_at")]
    require(all(t.tzinfo for t in times) and times == sorted(times), "runner times unordered")
    coordination = receipt["coordination"]
    require(coordination["scope"] == "cooperative" and all(Path(coordination[k]).resolve() == Path(v).resolve()
            for k, v in (("lock_file", "/tmp/pagura-bench.lock"), ("lock_directory", "/tmp/pagura-bench.lockdir"))),
            "wrong cooperative lock")
    require(guard["schema"] == 1 and guard["status"] == "completed" and guard["observation_only"] is False
            and guard["exit_code"] == guard["child_exit_code"] == 0 and guard["poll_seconds"] == 5
            and guard["window_seconds"] == 5400 and guard["command"] == command, "guard/producer mismatch")
    expected_argv = ["python3", "-B", "benchmarks/results/sources/analytic19-conversion/load_observer.py",
                     "--evidence-dir", str(Path(pin["guard"]["path"]).parent.relative_to(root)), "--", *command]
    require(receipt["argv"] == expected_argv, "runner/guard launch mismatch")
    require(receipt["artifacts"].get(report_pin["path"]) == report_pin["sha256"]
            and receipt["artifacts"].get(pin["guard"]["path"]) == pin["guard"]["sha256"], "final receipt does not bind report/guard")
    for path, digest in receipt["artifacts"].items():
        bound({"path": path, "sha256": digest})
    require([event["stage"] for event in guard["events"]] == ["preflight", "postflight"]
            and all(event["matches"] == [] for event in guard["events"]), "contention or incomplete guard")
    snapshot_paths = {str(within(Path(pin["guard"]["path"]).parent, e["snapshot"])) for e in guard["events"]}
    require(set(pin["snapshots"]) == snapshot_paths, "observer snapshots not independently pinned")
    for path, digest in pin["snapshots"].items():
        require(bool(bound({"path": path, "sha256": digest})), "empty observer snapshot")


def audit(pins):
    root = Path(pins["root"]).resolve()
    plan = json.loads(bound(pins["plan"]))
    bindings = json.loads(bound(pins["bindings"]))
    require(plan["schema"] == 1 and plan["experiment"] == "f32-preservation-cost"
            and plan["cases"] == list(CASES) and plan["modes"] == ["exact", "competitive"]
            and plan["operations"] == ["cold"] and plan["dpi"] == 72
            and (plan["cells"], plan["preflight_outputs"], plan["cost_outputs"], plan["seed"],
                 plan["warmup_pairs_per_cell"], plan["measured_pairs_per_cell"], plan["cost_trials"])
                == (38, 76, 2736, 20261110, 5, 31, 1)
            and plan["automatic_postlaunch_retry"] is False and plan["decision"]["admission"] is False,
            "frozen plan changed")
    require(bindings["schema"] == 1 and bindings["plan"] == PLAN and set(bindings["identities"]) == set(SIDES), "bindings mismatch")
    identities = bindings["identities"]
    require(identities["baseline"]["commit"] == plan["baseline_commit"] == "c70b467882faafc9baede8c06c7011ac2be1ae21"
            and identities["candidate"]["root"] == str(root), "wrong source arms")
    for identity in identities.values():
        require(source(Path(identity["root"])) == {k: identity[k] for k in ("root", "commit", "source_sha256")}, "current source drift")
        bound({"path": identity["binary"], "sha256": identity["binary_sha256"]})
    harness = bindings["harness_sha256"]
    expected_harness = {str(root / p) for p in ("benchmarks/results/sources/f32-preservation/cost.py",
        "benchmarks/paired_render.py", "benchmarks/color_reuse_fixtures.py",
        "benchmarks/results/sources/analytic19-conversion/manifest.json",
        "benchmarks/results/sources/analytic19-conversion/references.json")}
    require(set(harness) == expected_harness, "harness closure mismatch")
    for path, digest in harness.items():
        bound({"path": path, "sha256": digest})
    for path, digest in pins["coordination_sources"].items():
        bound({"path": path, "sha256": digest})
    require(set(pins["coordination_sources"]) == {str(root / p) for p in (
        "tools/runner.py", "benchmarks/results/sources/analytic19-conversion/load_observer.py")}, "coordination source closure")
    build = json.loads(bound(pins["build"]))
    require(build["status"] == "built-both-arms" and build["identities"] == identities, "build/source/binary binding mismatch")
    for side in SIDES:
        item = build["builds"][side]
        expected_source = {k: identities[side][k] for k in ("root", "commit", "source_sha256")}
        require(item["source_before"] == item["source_after"] == expected_source
                and item["process"]["exit_code"] == 0 and item["process"]["status"] != "timeout", "failed or drifting build")
        raw = bound({"path": str(Path(pins["build"]["path"]).parent / (side + ".log")), "sha256": item["log_sha256"]})
        cargo = [json.loads(line) for line in raw.splitlines() if line.startswith(b"{")]
        require(item["cargo_artifact"] in cargo and item["cargo_artifact"]["executable"]
                == str(Path(identities[side]["root"]) / "target/release/examples/scene_index_profile"), "native Cargo artifact mismatch")
    manifest = json.loads(bound({"path": str(root / "benchmarks/results/sources/analytic19-conversion/manifest.json"), "sha256": MANIFEST}))
    references = json.loads(bound({"path": str(root / "benchmarks/results/sources/analytic19-conversion/references.json"), "sha256": REFERENCES}))
    require([d["id"] for d in manifest["documents"]] == [d["id"] for d in references["cases"]] == list(CASES), "reference matrix mismatch")
    expected, inputs = {}, {}
    for document, ref in zip(manifest["documents"], references["cases"]):
        require(document["pages"] == 1 and document["selected_pages"] == [0] and len(document["oracles"]) == 1, "wrong reference page")
        oracle = document["oracles"][0]
        require((oracle["page"], oracle["dpi"], oracle["tolerance"]) == (0, 72, 0)
                and ref["expected"] == "render" and ref["dpi"] == 72
                and (ref["path"], ref["sha256"], ref["oracle"], ref["oracle_sha256"])
                == (document["path"], document["sha256"], oracle["path"], oracle["sha256"]), "reference identity/policy mismatch")
        pdf = bound({"path": str(within(root, document["path"])), "sha256": document["sha256"]})
        raw = bound({"path": str(within(root, oracle["path"])), "sha256": oracle["sha256"]})
        expected[document["id"]] = ppm(raw)
        require(expected[document["id"]][:2] == (ref["width"], ref["height"]), "reference dimensions mismatch")
        inputs[document["id"]] = {"pdf": "blobs/" + SHA(pdf), "oracle": "blobs/" + SHA(raw)}
    runner_identity, anchors, summaries = source(root, True), {}, []
    for phase in ("preflight", "cost"):
        pin = pins[phase]
        report_raw = bound(pin["report"])
        report, directory = json.loads(report_raw), Path(pin["report"]["path"]).parent
        require(report["schema"] == 1 and report["phase"] == phase and report["experiment"] == "f32-preservation-cost"
                and report["plan"] == PLAN and report["admission"] is False
                and report["identities"] == identities and report["harness_sha256"] == harness
                and report["bindings_sha256"] == pins["bindings"]["sha256"] and report["inputs"] == inputs, "report identity mismatch")
        require(report["status"] == ("quality-ready" if phase == "preflight" else "completed-diagnostic"), "failed/incomplete trial retained; no completed audit")
        blobs, stored = {}, 0
        for relative, item in report["artifacts"].items():
            require(relative == "blobs/" + item["sha256"], "artifact is not content addressed")
            raw = read(within(directory, relative))
            require(SHA(raw) == item["sha256"] and len(raw) == item["bytes"], "artifact digest/length mismatch")
            blobs[relative], stored = raw, stored + len(raw)
        require(stored == report["stored_blob_bytes"] and stored <= 512 * 1024 * 1024, "artifact accounting mismatch")
        require(blobs[report["bindings"]] == bound(pins["bindings"]), "retained bindings mismatch")
        require([{k: e[k] for k in ("path", "sha256")} for e in report["evidence"]] == bindings["evidence"] and bindings["evidence"], "missing build evidence")
        for evidence in report["evidence"]:
            require(blobs[evidence["artifact"]] == bound({k: evidence[k] for k in ("path", "sha256")}), "retained evidence mismatch")
        require(any(e["path"] == pins["build"]["path"] and e["sha256"] == pins["build"]["sha256"] for e in report["evidence"]), "build report omitted from frozen evidence")
        for item in inputs.values():
            require(item["pdf"] in blobs and item["oracle"] in blobs, "missing input blob")
        raw = read(directory / "records.jsonl")
        require(SHA(raw) == report["records_sha256"], "record checkpoint mismatch")
        rows = [json.loads(line) for line in raw.splitlines()]
        ordering = [(list(c), False, 0, s) for c in CELLS for s in SIDES]
        if phase == "cost":
            require(blobs[report["preflight"]] == bound(pins["preflight"]["report"]), "preflight report not bound")
            require(json.loads(blobs[report["schedule"]]) == schedule(), "randomized schedule mismatch")
            require((report["seed"], report["warmup_pairs"], report["measured_pairs"]) == (20261110, 5, 31), "trial parameters mismatch")
            ordering = [(c, warm, pair, side) for c, warm, pair, order in schedule() for side in order]
        else:
            require(report["samples"] == rows, "preflight samples differ from raw records")
        require(len(rows) == report["completed_records"] == len(ordering), "missing/extra raw records")
        require(report["failure_records"] == [], "failed records cannot be removed from completed trial")
        mismatches = []
        for row, (cell, warm, pair, side) in zip(rows, ordering):
            require((row["cell"], row["warmup"], row["pair"], row["side"]) == (cell, warm, pair, side)
                    and type(row["warmup"]) is bool and type(row["pair"]) is int, "record matrix/order mismatch")
            require(row["ready"] is True and "error" not in row, "failed sample retained; no successful audit")
            command = [identities[side]["binary"], str(directory / inputs[cell[0]]["pdf"]), cell[1], "cold", str(directory / "pending.rgba"), "72"]
            require(row["command"] == command, "renderer argv mismatch")
            raw = blobs[row["output"]]
            require(SHA(raw) == row["output_sha256"], "raw output binding mismatch")
            measured, differing_values = quality(raw, expected[cell[0]])
            require(row["quality"] == measured, "reported quality differs from raw bytes")
            if side == "candidate" or tuple(cell) != KNOWN:
                require(measured["rgb_equal"], "unexpected independent RGB failure")
            else:
                require((measured["max_channel_error"], measured["different_channels"], differing_values) == (1, 192, [(63, 64)]), "known baseline mismatch drift")
            if side == "baseline":
                if phase == "preflight":
                    anchors[tuple(cell)] = row["output_sha256"]
                    if not measured["rgb_equal"]:
                        mismatches.append({"cell": cell, "quality": measured})
                else:
                    require(row["output_sha256"] == anchors[tuple(cell)] and row["baseline_preflight_equal"] is True, "baseline output changed")
            diagnostic = blobs[row["diagnostic"]].decode("utf-8", errors="strict")
            stages = []
            for line in diagnostic.splitlines():
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict) and value.get("schema") == 1:
                    stages.append(value)
            require(stages == [row["stages"]], "missing/ambiguous/raw stage record mismatch")
            stage, process = row["stages"], row["process"]
            require(all(type(stage[k]) is int and stage[k] > 0 for k in ("open_ns", "compile_ns", "render_ns", "total_ns", "metric_ns")), "invalid stage measurements")
            require((stage["operation"], stage["accuracy"], stage["dpi"], stage["width"], stage["height"])
                    == ("cold", cell[1], 72, *expected[cell[0]][:2]) and stage["metric_ns"] == stage["total_ns"]
                    and stage["total_ns"] >= sum(stage[k] for k in ("open_ns", "compile_ns", "render_ns")), "stage settings/total mismatch")
            rss = re.findall(r"(?m)^\s*([0-9]+)\s+maximum resident set size\s*$", diagnostic)
            require(len(rss) == 1 and int(rss[0]) == process["peak_rss_bytes"] and process["rss_status"] == "available", "raw Darwin RSS mismatch")
            require(type(process["exit_code"]) is int and process["exit_code"] == 0 and process["status"] == "failed"
                    and all(type(process[k]) is int and process[k] > 0 for k in ("elapsed_ns", "peak_rss_bytes")), "invalid process result")
        command = producer_command(pin, root, phase, directory, pins)
        if phase == "preflight":
            require(report["baseline_rgb_mismatches"] == mismatches and len(mismatches) == 1, "baseline failure ledger mismatch")
        else:
            for cell in CELLS:
                selected = [r for r in rows if r["cell"] == list(cell)]
                summary = {"cell": list(cell), "baseline_matches_oracle": cell != KNOWN, "complete_and_valid": True,
                    "medians": {side: {metric: statistics.median((r["stages"] if metric in ("total_ns", "compile_ns", "render_ns") else r["process"])[metric]
                        for r in selected if r["side"] == side and not r["warmup"])
                        for metric in ("total_ns", "compile_ns", "render_ns", "elapsed_ns", "peak_rss_bytes")} for side in SIDES},
                    "measured_maximum_peak_rss_bytes": {side: max(r["process"]["peak_rss_bytes"] for r in selected if r["side"] == side and not r["warmup"]) for side in SIDES}}
                summaries.append(summary)
            require(report["summary"] == summaries, "recomputed summary mismatch")
        final_coordination(pin, report, pin["report"], root, runner_identity, command)
    return {"schema": 1, "status": "audited-completed-diagnostic", "admission": False,
            "preflight_outputs": 76, "cost_outputs": 2736, "baseline_known_mismatch": list(KNOWN),
            "summary": summaries, "limitations": ["Descriptive accuracy cost; no admission or equivalent-output superiority",
            "Frozen independent analytic19 fixture models only", "Known-process observer, not hardware exclusivity",
            "Current source and retained build identities checked; no rebuild or signature attestation",
            "Raw Darwin RSS/stage records rechecked; process elapsed_ns is retained measurement, not reconstructible from pixels"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pins", type=Path, required=True)
    parser.add_argument("--pins-sha256", required=True)
    args = parser.parse_args()
    try:
        raw = bound({"path": str(args.pins.resolve()), "sha256": args.pins_sha256})
        result = audit(json.loads(raw))
        result.update(pins_sha256=SHA(raw), auditor_sha256=SHA(read(__file__)))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "audit-failed", "admission": False, "error": f"{type(error).__name__}: {error}"}))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
