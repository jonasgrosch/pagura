"""Unattended, offline round-5 conversion matrix. See benchmarks/README.md."""
import argparse
import collections
import json
import os
import random
import shutil
import statistics
import subprocess
import sys
import time
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from benchkit.config import POLICY_PATH, read_json, sha, source_digest
from benchkit.corpus import corpus_cases
from benchkit.documents import GOOD, METRICS, _statistics, document_command, document_jobs, expected_pages, measure_document
from benchkit.engines import engines, identify
from benchkit.images import normalize
from benchkit.reports import new_report, write_summary
import benchkit.documents as documents

from agreement import pixel_difference
from machine_record import snapshot
from process_measurement import measurement_options, run_process
import round5_compare as compare

NATIVE = tuple(name for name in ("pagura", "pdfium", "mupdf", "quartz", "hayro", "pdfjs", "poppler")
               if name != "quartz" or sys.platform == "darwin")
WASM = ("v8-wasi", "wasmtime")
# Eager compilation keeps V8's optimizing compile at instantiation, before the stage timers,
# as wasmtime's AOT artifact does; lazy compilation would compile each function on first call.
V8_FLAGS = ("--single-threaded", "--no-liftoff", "--no-wasm-lazy-compilation", "--wasm-num-compilation-tasks=1")
V8_NOTE = ("V8 compiles the whole module at instantiation (`--no-wasm-lazy-compilation`), before the stage timers, "
           "as wasmtime does ahead of time. Runs before 2026-10-19 compiled each function lazily inside the timed "
           "stages, adding about 0.7-1 s to them; process wall time now includes compiling the whole module instead. "
           "Compare V8 cells only within one setting.")
CONTENT = ("text-heavy", "image-heavy", "vector-heavy", "mixed")
X86_V3 = "pagura-x86-64-v3"
# The competitive accuracy path (ADR 0004) of the same portable binary, as a row of its own.
COMPETITIVE = "pagura-competitive"
COMPETITIVE_FLAGS = ("--accuracy", "competitive")


def target_features(toolchain, host, env, *flags):
    """Record enabled compiler features, rather than all supported features."""
    cfg = subprocess.check_output(toolchain + ["rustc", "--print", "cfg", "--target", host, *flags],
                                  cwd=ROOT, env=env, text=True)
    return sorted(line.removeprefix('target_feature="').removesuffix('"')
                  for line in cfg.splitlines() if line.startswith('target_feature="'))


def build_x86_v3(output, toolchain, host, env):
    """Keep the optional ISA track isolated from the portable headline build."""
    track = {"status": "unavailable", "target": host, "rustflags": "-C target-cpu=x86-64-v3"}
    if not host.startswith("x86_64-"):
        return {**track, "reason": "x86-64-v3 requires an x86_64 host"}
    track["target_features"] = target_features(toolchain, host, env, "-C", "target-cpu=x86-64-v3")
    native_features = target_features(toolchain, host, env, "-C", "target-cpu=native")
    track["host_target_features"] = native_features
    missing = set(track["target_features"]) - set(native_features)
    if missing:
        return {**track, "reason": "host lacks x86-64-v3 features: " + ", ".join(sorted(missing))}
    variant_env = dict(env, CARGO_TARGET_DIR=str(output / "build-x86-64-v3"),
                       RUSTFLAGS=track["rustflags"])
    variant_env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    checked(toolchain + ["cargo", "build", "--release", "--locked", "--offline", "--target", host,
                         "--bin", "pagura", "--features", "bundled-data"], output / "build.log", env=variant_env)
    executable = "pagura.exe" if "windows" in host else "pagura"
    binary = output / "build-x86-64-v3" / host / "release" / executable
    return {**track, "status": "available", "binary": str(binary)}


def required(name):
    value = os.environ.get(name)
    if not value:
        raise ValueError(f"missing environment variable {name}")
    return value


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".pending")
    temporary.write_text(json.dumps(value, separators=(",", ":")) + "\n")
    temporary.replace(path)


def harness_sources():
    return {str(path.relative_to(ROOT)): sha(path.read_bytes()) for path in
            sorted((ROOT / "benchmarks").glob("*.py"))}


def remaining_time(args):
    remaining = args.deadline - time.monotonic()
    if remaining <= 0:
        raise RuntimeError("measurement time box exhausted; partial cells retained, no cells retried")
    return remaining


def checked(command, log, *, env=None):
    with log.open("a") as output:
        output.write(json.dumps(list(map(str, command))) + "\n")
        output.flush()
        subprocess.run(list(map(str, command)), cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                       env=env, check=True, timeout=900)


def build(output, wasmtime, *, include_wasm=True, x86_v3=False):
    """Offline clean targets and explicit identical bundled-data features."""
    artifacts = output / "artifacts"
    artifacts.mkdir()
    target = output / "build"
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    toolchain = ["rustup", "run", channel]  # no --install: a missing toolchain fails offline
    info = subprocess.check_output(toolchain + ["rustc", "-vV"], cwd=ROOT, text=True)
    host = next(line.removeprefix("host: ") for line in info.splitlines() if line.startswith("host: "))
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        env.pop(name, None)
    env["CARGO_ENCODED_RUSTFLAGS"] = ""  # override per-target/global Cargo config too
    features = target_features(toolchain, host, env)
    log = output / "build.log"
    checked(toolchain + ["cargo", "build", "--release", "--locked", "--offline", "--target", host,
             "--bin", "pagura", "--bin", "pagura-host", "--features", "pagura-host/bundled-data"], log, env=env)
    wasi_env = dict(env, CARGO_ENCODED_RUSTFLAGS="-C\x1ftarget-feature=+simd128")
    if include_wasm:
        checked(toolchain + ["cargo", "build", "--release", "--locked", "--offline", "-p", "pagura-host",
                 "--features", "bundled-data", "--target", "wasm32-wasip1"], log, env=wasi_env)
    targets = [(host, "native-control")] + ([("wasm32-wasip1", "convert.wasm")] if include_wasm else [])
    for triple, name in targets:
        deps = target / triple / "release/deps"
        libraries = list(deps.glob("libpagura-*.rlib"))
        if len(libraries) != 1:
            raise ValueError("ambiguous facade library in clean benchmark target")
        command = toolchain + ["rustc", "--edition=2024", "--target", triple, "-O", "-C", "lto=thin",
                   "-C", "codegen-units=1", "-L", f"dependency={deps}", "--extern", f"pagura={libraries[0]}",
                   ROOT / "benchmarks/adapters/wasm_convert.rs", "-o", artifacts / name]
        if triple == "wasm32-wasip1":
            command += ["-C", "target-feature=+simd128"]
        checked(command, log, env=env)
    if include_wasm:
        checked([wasmtime, "compile", "-C", "cache=n", "-C", "parallel-compilation=n",
                 artifacts / "convert.wasm", "-o", artifacts / "convert.cwasm"], log)
    tracks = {"pagura": {"status": "available", "target": host,
                         "target_features": features}}
    if x86_v3:
        tracks[X86_V3] = build_x86_v3(output, toolchain, host, env)
    atomic_json(output / "compiler.json", {"rustc": info, "host": host, "native_tracks": tracks,
        "features": ["bundled-data"], "native_rustflags": "target baseline; inherited RUSTFLAGS cleared",
        "native_target_features": features,
        "wasm_target_features": target_features(toolchain, "wasm32-wasip1", env, "-C", "target-feature=+simd128") if include_wasm else None,
        "wasm_rustflags": "-C target-feature=+simd128" if include_wasm else None,
        "wasmtime": "Cranelift AOT opt-level=2; parallel-compilation=n" if include_wasm else None,
        "product_host_wasm_sha256": sha((target / "wasm32-wasip1/release/pagura-host.wasm").read_bytes()) if include_wasm else None})
    return target / host / "release/pagura", artifacts


def native_simd(host, features):
    """Describe compiler-enabled SIMD, rather than the hardware's capabilities."""
    if host.startswith("x86_64-"):
        enabled = ("sse2" in features, "avx2" in features, any(item.startswith("avx512") for item in features))
        detail = ", ".join(f"{name} {'enabled' if present else 'disabled'}"
                           for name, present in zip(("SSE2", "AVX2", "AVX-512"), enabled))
    elif host.startswith("aarch64-"):
        detail = "NEON enabled" if "neon" in features else "NEON disabled"
    else:
        detail = "target features: " + (", ".join(features) or "none")
    return f"compiler autovectorization at {host} target baseline: {detail}"


def page_metadata(manifest):
    metadata = {}
    for document in manifest["documents"]:
        selection = {entry["page"]: entry for entry in document.get("selection", [])}
        for page in document["selected_pages"]:
            item = selection.get(page)
            if not item or item.get("content") not in CONTENT or item.get("size") not in ("small", "standard", "large") or not isinstance(item.get("producer"), str) or not isinstance(item.get("reason"), str):
                raise ValueError("round-4 manifest requires content, size, producer and reason for every selected page")
            metadata[document["id"], page] = item
    if len(metadata) != 40 or len(manifest["documents"]) < 20:
        raise ValueError("expected round-4 manifest: 40 selected pages from at least 20 files")
    if collections.Counter(item["content"] for item in metadata.values()) != {name: 10 for name in CONTENT}:
        raise ValueError("expected ten round-4 pages per content category")
    return metadata


def subset_300(cases, metadata, geometry, seed):
    """Predeclared 20-file/producer subset, independent of measured performance.

    Five pages per content; one small in each; one large in text/image/mixed;
    remaining pages standard. The oversized vector plan is outside the existing
    16-million-pixel image budget, which is never relaxed for this track.
    """
    candidates = list(cases)
    random.Random(seed).shuffle(candidates)
    chosen, files, producers = [], set(), set()
    for content in CONTENT:
        quotas = (("large", 0 if content == "vector-heavy" else 1), ("small", 1),
                  ("standard", 4 if content == "vector-heavy" else 3))
        for size, count in quotas:
            eligible = [case for case in candidates if metadata[case["id"], case["page"]]["content"] == content
                        and metadata[case["id"], case["page"]]["size"] == size]
            selected = 0
            for case in eligible:
                item = metadata[case["id"], case["page"]]
                width, height = geometry[case["id"], case["page"], 300]
                if selected == count:
                    break
                if case["id"] in files or item["producer"] in producers or width * height > 16_000_000:
                    continue
                chosen.append({**case, "dpi": 300})
                files.add(case["id"])
                producers.add(item["producer"])
                selected += 1
            if selected != count:
                raise ValueError(f"cannot satisfy predeclared 300-dpi {content}/{size} stratum")
    return chosen


def wasm_command(name, node, wasmtime, artifacts, pages, output):
    case = pages[0]
    source = Path(case["path"])
    args = [str(case["dpi"]), str(pages[0]["page"]), str(pages[-1]["page"])]
    if "password" in case:
        args += ["--password", case["password"]]
    if name == "v8-wasi":
        return [node, *V8_FLAGS,
                str(ROOT / "benchmarks/adapters/wasi.mjs"), str(artifacts / "convert.wasm"), str(source), str(output), *args]
    if "::" in str(source.parent) or "::" in str(output):
        raise ValueError("Wasmtime directory mappings cannot contain '::'")
    return [wasmtime, "run", "-C", "cache=n", "-C", "parallel-compilation=n", "--allow-precompiled",
            "--dir", f"{source.parent}::/input", "--dir", f"{output}::/output",
            str(artifacts / "convert.cwasm"), f"/input/{source.name}", "/output", *args]


def parity(native, wasm):
    return (native.get("status") in GOOD and wasm.get("status") in GOOD
            and (native.get("width"), native.get("height"), native.get("pixels_sha256")) ==
                (wasm.get("width"), wasm.get("height"), wasm.get("pixels_sha256")))


def verify_runtime_parity(native, wasm):
    match = parity(native, wasm)
    if not match and native.get("status") in GOOD and wasm.get("status") in GOOD:
        wasm["status"] = "runtime-parity-mismatch"
    return match


def failure_sink(directory):
    """Keep the shared verifier's bounded failure evidence inside this run."""
    def preserve(path, result, policy, label):
        if result["status"] in GOOD:
            return
        try:
            if path.is_file() and path.stat().st_size <= policy["maximum_output_bytes"]:
                directory.mkdir(exist_ok=True)
                target = directory / (sha(label.encode()) + ".raw")
                shutil.copyfile(path, target)
                result["artifact"] = str(target)
        except OSError as error:
            result["artifact_error"] = str(error)
    return preserve


def idle_record(machine, path, node, *, smoke):
    for attempt in range(4):
        record = snapshot(node)
        machine["observations"].append(record)
        atomic_json(path, machine)
        if not record["heavy_processes"] or smoke:
            return
        if attempt == 3:
            raise RuntimeError("other heavy processes persist after three 30-second pauses; raw evidence retained")
        print("Other heavy processes observed; pausing 30 seconds", flush=True)
        machine["pauses"].append({"utc": record["utc"], "seconds": 30, "processes": record["heavy_processes"]})
        time.sleep(30)


def timed_engines(selected, bases):
    """The optional x86-64-v3 build and competitive path are Pagura rows, timed whenever portable Pagura is."""
    rows = tuple(name for name in (X86_V3, COMPETITIVE) if name in bases)
    if not rows or "pagura" not in selected:
        return selected
    native = tuple(name for name in selected if name not in WASM)
    return (*native, *rows, *selected[len(native):])


def measurement(report, jobs, bases, node, wasmtime, artifacts, policy, output, machine, args):
    """Each engine runs exactly once per complete block; no retry/filtering."""
    rng = random.Random(args.seed)
    rng.shuffle(jobs)
    selected_names = timed_engines(args.engines or (*NATIVE, *WASM), bases)
    parity_names = (*WASM, X86_V3) if X86_V3 in bases else WASM
    documents.run_process = run_process
    documents.preserve_failure = failure_sink(output / "failures")
    temporary = output / "temporary"
    temporary.mkdir()
    representatives = output / "representatives"
    representatives.mkdir()
    with (output / "blocks.jsonl").open("x") as blocks, (output / "processes.jsonl").open("x") as processes:
        for index, pages in enumerate(jobs):
            remaining_time(args)
            idle_record(machine, output / "machine.json", node, smoke=args.smoke)
            case = pages[0]
            names = tuple(name for name in selected_names if (case["id"], case["dpi"]) in args.main_jobs)
            if (case["id"], case["dpi"]) in args.control_jobs and "hayro" not in names:
                names += ("hayro",)
            baselines = tuple(name for name in names if name != "pagura") if "pagura" in names else ()
            if sha(Path(case["path"]).read_bytes()) != case["sha256"]:
                raise ValueError("PDF changed after manifest validation")
            expected = expected_pages(pages)
            cells, docs, retained = {}, {}, {}
            for engine in baselines:
                common = {"case": case["id"], "dpi": case["dpi"], "engine": engine, "operation": "conversion",
                          "pdf_sha256": case["sha256"], "status": "measured", "verification": "unverified", "commands": {}}
                docs[engine] = {**common, "pages": list(expected), "samples": [], "warmup_failures": []}
                report["document_cells"].append(docs[engine])
                for page in expected:
                    cells[engine, page] = {**common, "page": page, "oracle_pixels_sha256": None,
                                          "tolerance": 0, "samples": [], "warmup_failures": []}
                    report["cells"].append(cells[engine, page])
            for pair in range(-args.warmups, args.samples):
                remaining_time(args)
                order = list(names)
                rng.shuffle(order)
                block_id = f"{index}/{pair}"
                runs, commands, images = {}, {}, {}
                for name in order:
                    timeout = min(policy["timeout_seconds"], remaining_time(args))
                    destination = temporary / name
                    destination.mkdir()
                    command = (document_command("pagura" if name in (X86_V3, COMPETITIVE) else name, bases[name], pages, destination) if name in bases else
                               wasm_command(name, node, wasmtime, artifacts, pages, destination))
                    decoded = {}
                    run = measure_document(command, destination, expected, {**policy, "timeout_seconds": timeout},
                                           stages_required=name != "poppler", image_sink=decoded)
                    processes.write(json.dumps({"block_id": block_id, "case": case["id"], "dpi": case["dpi"],
                        "pair": pair, "engine": name, "command": command, "run": run}, separators=(",", ":")) + "\n")
                    processes.flush()
                    runs[name], commands[name], images[name] = run, command, decoded
                    if pair >= 0:
                        for page, path in decoded.items():
                            if (name, page) not in retained:
                                target = representatives / (sha(f"{case['id']}/{page}/{case['dpi']}/{name}".encode()) + ".ppm")
                                width, height, pixels = normalize(path.read_bytes())
                                target.write_bytes(f"P6\n{width} {height}\n255\n".encode() + pixels)
                                retained[name, page] = target
                by_page = {name: {page["page"]: page for page in run["pages"]} for name, run in runs.items()}
                for name in parity_names:
                    if name not in runs or "pagura" not in runs:
                        continue
                    for page in expected:
                        match = verify_runtime_parity(by_page["pagura"][page], by_page[name][page])
                        comparison = {"block_id": block_id, "case": case["id"], "dpi": case["dpi"], "page": page, "engine": name,
                            "exact": match, "native_pixels_sha256": by_page["pagura"][page].get("pixels_sha256"),
                            "comparison_pixels_sha256": by_page[name][page].get("pixels_sha256")}
                        if name in WASM:
                            comparison["wasm_pixels_sha256"] = comparison["comparison_pixels_sha256"]
                        report["runtime_parity"].append(comparison)
                        if not match:
                            # Failed parity suppresses ratios. It does not call either engine a PDF oracle.
                            source = images[name].get(page)
                            if source:
                                shutil.copyfile(source, representatives / (sha(f"parity/{block_id}/{name}/{page}".encode()) + ".raw"))
                            if by_page[name][page]["status"] == "runtime-parity-mismatch":
                                runs[name]["status"] = "runtime-parity-mismatch"
                block = {"block_id": block_id, "case": case["id"], "dpi": case["dpi"], "pair": pair,
                         "order": order, "commands": commands, "runs": runs}
                blocks.write(json.dumps(block, separators=(",", ":")) + "\n")
                blocks.flush()
                for engine in baselines:
                    paired_order = [name for name in order if name in ("pagura", engine)]
                    docs[engine]["commands"] = {name: commands[name] for name in ("pagura", engine)}
                    sample = {"pair": pair, "block_id": block_id, "order": paired_order,
                              "pagura": runs["pagura"], "baseline": runs[engine]}
                    if pair >= 0:
                        docs[engine]["samples"].append(sample)
                    elif any(sample[key]["status"] not in GOOD for key in ("pagura", "baseline")):
                        docs[engine]["warmup_failures"].append(sample)
                    for page in expected:
                        sample_page = {"pair": pair, "block_id": block_id, "order": paired_order,
                                       "pagura": by_page["pagura"][page], "baseline": by_page[engine][page]}
                        if pair >= 0:
                            cells[engine, page]["samples"].append(sample_page)
                        elif any(sample_page[key]["status"] not in GOOD for key in ("pagura", "baseline")):
                            cells[engine, page]["warmup_failures"].append(sample_page)
                for name in names:
                    shutil.rmtree(temporary / name)
                print(f"Job {index + 1}/{len(jobs)}, block {pair}: measured" if pair >= 0 else
                      f"Job {index + 1}/{len(jobs)}, warmup {pair}", flush=True)
            for page in expected:
                if ("pagura", page) not in retained:
                    continue
                first = normalize(retained["pagura", page].read_bytes())
                for engine in baselines:
                    if (engine, page) not in retained:
                        continue
                    difference = pixel_difference(first, normalize(retained[engine, page].read_bytes()))
                    outlier = (difference["status"] != "comparable" or difference.get("mean_channel_error", 0) >= 10 or
                        (difference.get("max_channel_error", 0) >= 200 and difference.get("different_channels", 0) / (first[0] * first[1] * 3) >= 0.02))
                    report["agreement"].append({"case": case["id"], "page": page, "dpi": case["dpi"],
                        "engines": ["pagura", engine], "scope": "first decoded measured output; investigation, not correctness",
                        "paths": [str(retained["pagura", page]), str(retained[engine, page])], "outlier_for_visual_review": outlier, **difference})
            report["completed_jobs"] = index + 1
            if sha(Path(case["path"]).read_bytes()) != case["sha256"]:
                raise ValueError("PDF changed during a document/DPI job")
            atomic_json(output / "conversion.json", report)
            print(f"Completed document/DPI job {index + 1}/{len(jobs)}", flush=True)
    temporary.rmdir()


def cpu_summary(output):
    samples = collections.defaultdict(list)
    warmups = collections.defaultdict(list)
    with (output / "blocks.jsonl").open() as records:
        for line in records:
            block = json.loads(line)
            for engine, run in block["runs"].items():
                (samples if block["pair"] >= 0 else warmups)[engine].append(run)
    summary = {}
    for name, runs in samples.items():
        ratios = [run["cpu_wall_ratio"] for run in runs if run.get("cpu_wall_ratio") is not None]
        rss = [run["peak_rss_bytes"] for run in runs if run.get("peak_rss_bytes") is not None]
        all_ratios = ratios + [run["cpu_wall_ratio"] for run in warmups[name] if run.get("cpu_wall_ratio") is not None]
        all_rss = rss + [run["peak_rss_bytes"] for run in warmups[name] if run.get("peak_rss_bytes") is not None]
        summary[name] = {"samples": len(runs), "accounted_samples": len(ratios),
            "median_cpu_wall": statistics.median(ratios) if ratios else None,
            "max_cpu_wall": max(ratios) if ratios else None,
            "samples_above_1_05": sum(run.get("cpu_parallelism_flag", False) for run in runs),
            "warmup_samples": len(warmups[name]),
            "warmup_samples_above_1_05": sum(run.get("cpu_parallelism_flag", False) for run in warmups[name]),
            "max_cpu_wall_including_warmups": max(all_ratios) if all_ratios else None,
            "peak_rss_bytes": max(rss) if rss else None,
            "peak_rss_bytes_including_warmups": max(all_rss) if all_rss else None,
            "statuses": dict(collections.Counter(run["status"] for run in runs))}
    return summary


def engine_names(value):
    names = value.split(",")
    allowed = (*NATIVE, *WASM)
    if not names or len(set(names)) != len(names) or any(name not in allowed for name in names):
        raise argparse.ArgumentTypeError("engines must be distinct names from: " + ",".join(allowed))
    return tuple(name for name in allowed if name in names)


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--smoke", action="store_true", help="two pages, 72 dpi, three samples and one warmup")
    parser.add_argument("--pagura-x86-64-v3", action="store_true",
                        help="add a separate x86-64-v3 row when the host supports it; portable Pagura stays the reference")
    parser.add_argument("--pagura-competitive", action="store_true",
                        help="add a separate row for the competitive accuracy path (RenderOptions::accuracy) "
                             "of the portable build; exact Pagura stays the reference")
    parser.add_argument("--dpi300-pages", type=int, choices=(20, 40), default=20)
    parser.add_argument("--seed", type=int, default=52018)
    parser.add_argument("--timebox-seconds", type=int, default=7200)
    parser.add_argument("--engines", type=engine_names, help="comma-separated engines; default: all applicable engines")
    parser.add_argument("--reference", type=Path, help="completed round-5 run on this machine and manifest")
    parser.add_argument("--control", type=int, default=0, help="fixed Hayro page/DPI controls against --reference")
    args = parser.parse_args(argv)
    if args.control < 0:
        parser.error("control must be nonnegative")
    if args.control and not args.reference:
        parser.error("control requires --reference")
    if args.reference and args.control < 1:
        parser.error("reference requires --control >= 1")
    if args.pagura_x86_64_v3 and args.engines is not None and "pagura" not in args.engines:
        parser.error("pagura-x86-64-v3 requires pagura in --engines as its parity reference")
    if args.pagura_competitive and args.engines is not None and "pagura" not in args.engines:
        parser.error("pagura-competitive requires pagura in --engines as its accuracy reference")
    if args.timebox_seconds <= 0:
        parser.error("timebox-seconds must be positive")
    return args


def result_exit_status(exact, failed, comparison_status=None):
    if comparison_status is not None and comparison_status != "stable":
        return 3
    return 0 if exact and not failed else 2


def main(argv=None):
    args = parse_args(argv)
    retiming = args.engines is not None or args.reference is not None or args.control != 0
    names = args.engines or (*NATIVE, *WASM)
    needed = tuple(dict.fromkeys((*names, *(("hayro",) if args.control else ()))))
    include_wasm = any(name in WASM for name in needed)
    args.deadline = time.monotonic() + args.timebox_seconds
    args.samples, args.warmups = (3, 1) if args.smoke else (10, 3)
    output = args.output.resolve()
    if not output.is_relative_to((ROOT / "benchmarks/results/local").resolve()):
        raise ValueError("output must be a new directory under benchmarks/results/local")
    manifest = Path(required("PAGURA_BENCH_MANIFEST")).expanduser().resolve()
    if manifest.is_relative_to(ROOT):
        raise ValueError("the private round-4 manifest must remain outside the repository")
    digest = sha(manifest.read_bytes())
    if digest != required("PAGURA_BENCH_MANIFEST_SHA256"):
        raise ValueError("round-4 manifest SHA-256 mismatch")
    corpus = Path(required("PAGURA_CORPUS_DIR")).expanduser().resolve()
    if corpus.is_relative_to(ROOT):
        raise ValueError("the real corpus must remain outside the repository")
    cases, _ = corpus_cases(corpus, manifest, dpis=(72,))
    metadata = page_metadata(read_json(manifest))
    node = os.environ.get("PAGURA_NODE", shutil.which("node") or "node")
    wasmtime = None
    if include_wasm:
        wasmtime = str(Path(required("PAGURA_WASMTIME")).expanduser().resolve())
        if subprocess.check_output([wasmtime, "--version"], text=True).split()[:2] != ["wasmtime", "49.0.3"]:
            raise ValueError("Wasmtime 49.0.3 is required")
        expected_wasmtime = os.environ.get("PAGURA_WASMTIME_SHA256")
        if expected_wasmtime and sha(Path(wasmtime).read_bytes()) != expected_wasmtime:
            raise ValueError("Wasmtime executable SHA-256 mismatch")
    # Installed only in the isolated benchmark environment.
    import PIL  # pyright: ignore[reportMissingImports]
    output.mkdir(parents=True, exist_ok=False)
    atomic_json(output / "status.json", {"status": "running", "smoke": args.smoke, "release_eligible": False})
    report = None
    try:
        pinned = source_digest()
        harness_before = harness_sources()
        checked([sys.executable, "-m", "unittest", "discover", "-s", "benchmarks", "-p", "test_*.py"], output / "preflight.log")
        machine = {"observations": [snapshot(node)], "pauses": []} if retiming else {"observations": [], "pauses": []}
        options = measurement_options() if args.reference else None
        protocol = {"samples_requested": args.samples, "warmups": args.warmups,
                    "policy_sha256": sha(POLICY_PATH.read_bytes()), "measurement_options": options,
                    "round5": {"harness_sha256": harness_before}}
        reference = compare.load_reference(args.reference, digest, machine["observations"][0], protocol) if args.reference else None
        controls = compare.select_controls(reference, args.control) if reference else []
        if reference:
            atomic_json(output / "reference.json", reference["provenance"])
        native, artifacts = build(output, wasmtime, include_wasm=include_wasm, x86_v3=args.pagura_x86_64_v3)
        compiler = read_json(output / "compiler.json")
        machine["compiler"] = compiler
        atomic_json(output / "machine.json", machine)
        if source_digest() != pinned or harness_sources() != harness_before:
            raise ValueError("source changed during build")
        bases = {name: engines()[name] for name in NATIVE if name in needed}
        if "pagura" in bases:
            bases["pagura"] = [str(native)]
        if "pdfjs" in bases:
            bases["pdfjs"] = [node, "--single-threaded", *bases["pdfjs"][1:]]
        identities = {name: identify(name, command) for name, command in bases.items()}
        if any(value["status"] != "available" for value in identities.values()):
            atomic_json(output / "engines.json", identities)
            raise ValueError("all applicable competitors must be installed and available; see engines.json")
        if args.pagura_x86_64_v3:
            track = compiler["native_tracks"][X86_V3]
            if track["status"] == "available":
                bases[X86_V3] = [track["binary"]]
                identities[X86_V3] = identify("pagura", bases[X86_V3])
                if identities[X86_V3]["status"] != "available":
                    raise ValueError("x86-64-v3 build failed its executable preflight")
            else:
                identities[X86_V3] = track
        if args.pagura_competitive and "pagura" in bases:
            bases[COMPETITIVE] = [*bases["pagura"], *COMPETITIVE_FLAGS]
            identities[COMPETITIVE] = identify("pagura", bases[COMPETITIVE])
            if identities[COMPETITIVE]["status"] != "available":
                raise ValueError("the competitive path failed its executable preflight")
        names = timed_engines(names, bases)
        for name, runtime in (("v8-wasi", node), ("wasmtime", wasmtime)):
            if name not in needed:
                continue
            assert runtime is not None, f"{name} is needed but was not located"
            paths = [Path(shutil.which(runtime) or runtime), artifacts / "convert.wasm", artifacts / "convert.cwasm",
                     ROOT / "benchmarks/adapters/wasm_convert.rs", ROOT / "benchmarks/adapters/wasi.mjs"]
            identities[name] = {"status": "available", "version": subprocess.check_output([runtime, "--version"], text=True).strip(),
                                "artifacts": {str(path): sha(path.read_bytes()) for path in paths}}
        atomic_json(output / "engines.json", identities)
        if controls:
            assert reference is not None, "controls come from the reference run"
            compare.check_control_identity(identities["hayro"], reference["report"]["engine_identities"]["hayro"])
        if "quartz" in bases:
            checked([sys.executable, ROOT / "benchmarks/adapters/quartz_geometry_test.py", bases["quartz"][0]], output / "preflight.log")
        if include_wasm:
            checked([sys.executable, ROOT / "benchmarks/adapters/wasm_convert_test.py", artifacts / "native-control",
                     artifacts / "convert.wasm", wasmtime], output / "preflight.log")
        geometry = {}
        for pages in document_jobs(cases, bases):
            for dpi in (72, 150, 300):
                command = [str(artifacts / "native-control"), "--geometry", pages[0]["path"], str(dpi),
                           str(pages[0]["page"]), str(pages[-1]["page"])]
                if "password" in pages[0]:
                    command += ["--password", pages[0]["password"]]
                for page in json.loads(subprocess.check_output(command, text=True))["pages"]:
                    geometry[pages[0]["id"], page["page"], dpi] = (page["width"], page["height"])
        high_resolution = (subset_300(cases, metadata, geometry, args.seed) if args.dpi300_pages == 20 else
                           [{**case, "dpi": 300} for case in cases])
        full_plan = cases + [{**case, "dpi": 150} for case in cases] + high_resolution
        if args.smoke:
            selected = [next(case for case in cases if metadata[case["id"], case["page"]]["content"] == content)
                        for content in ("text-heavy", "image-heavy")]
        else:
            selected = full_plan
        if reference:
            reference_selection = {(cell["case"], cell["page"], cell["dpi"]): cell for cell in reference["report"]["round5"]["selection"]}
            for case in selected:
                previous = reference_selection.get((case["id"], case["page"], case["dpi"]))
                if previous is None or previous["sha256"] != case["sha256"]:
                    raise ValueError("reference selection does not cover the requested page/DPI plan")
        args.main_jobs = {(case["id"], case["dpi"]) for case in selected}
        args.control_jobs = {(cell["case"], cell["dpi"]) for cell in controls}
        # Reference document timings are comparable only with the same page range.
        if reference:
            source_cases = {(case["id"], case["page"]): case for case in cases}
            planned = [{**source_cases[cell["case"], cell["page"]], "dpi": cell["dpi"]}
                       for cell in reference["report"]["round5"]["selection"]
                       if (cell["case"], cell["dpi"]) in args.main_jobs | args.control_jobs]
            jobs = document_jobs(planned, bases)
        else:
            jobs = document_jobs(selected, bases)
        report = new_report(identities, args.samples, args.warmups, args.seed, digest, "conversion", None, process_per_page=False)
        report["source_sha256"] = pinned
        report["suite"] = "round5-smoke" if args.smoke else "round5-real-documents"
        report.update(runtime_parity=[], completed_jobs=0, planned_jobs=len(jobs),
                      measurement_options=options if options is not None else measurement_options())
        report["round5"] = {"smoke": args.smoke, "manifest_external_path": str(manifest), "corpus_external_path": str(corpus),
            "validated_full_plan": {"page_dpi_cases": len(full_plan), "document_dpi_jobs": len(document_jobs(full_plan, bases)),
                                    "pages_at_300_dpi": len(high_resolution)},
            "sampling": "randomized complete blocks, one shared Pagura sample per block; comparisons correlated; no retries",
            "stage_boundaries": "open=load/parse/enumerate; render=load page+compile+paint; emit=convert/encode/write",
            "wasm_features": ["bundled-data", "simd128"], "v8_flags": list(V8_FLAGS),
            "wasmtime_flags": ["Cranelift AOT opt-level=2", "parallel-compilation=n"], "agreement_backend": f"Pillow {PIL.__version__}",
            "simd": {"pagura": native_simd(compiler["host"], compiler["native_target_features"]),
                     "native_tracks": compiler["native_tracks"], "wasm": "SIMD128",
                     "hayro": "runtime-detected SIMD; ISA dispatch not independently audited", "other_engines": "ISA dispatch not independently audited"},
            "threading": {"hayro": "0.8.0 forces num_threads=0; isolated vello_cpu multithreading feature disabled",
                          "pdfium_mupdf": "synchronous calling-thread render APIs",
                          "pdfjs": "Node main-thread path, synchronous raster Canvas; V8 background tasks disabled; native background work not independently ruled out",
                          "quartz_poppler": "internal threading unknown; every process CPU/wall flag retained",
                          "pagura_wasm": "single-thread engine; runtime/compiler settings recorded separately"},
            "dpi300_policy": "predeclared 20-page subset, 5/content, 4 small/13 standard/3 large, distinct files/producers" if args.dpi300_pages == 20 else "all 40 pages; existing image budgets still apply",
            "harness_sha256": harness_before,
            "selection": [{"case": case["id"], "page": case["page"], "dpi": case["dpi"], "sha256": case["sha256"],
                           **{key: metadata[case["id"], case["page"]][key] for key in ("content", "size", "producer", "reason")}}
                          for case in selected]}
        if retiming:
            report["retiming"] = {"engines": list(names), "control_cells": controls,
                                  "wasm_built": include_wasm,
                                  "reference": reference["provenance"] if reference else None}
        atomic_json(output / "conversion.json", report)
        policy = read_json(POLICY_PATH)
        measurement(report, jobs, bases, node, wasmtime, artifacts, policy, output, machine, args)
        machine["observations"].append(snapshot(node))
        atomic_json(output / "machine.json", machine)
        # Bootstrap work runs only after every timed cell is finished.
        for cell in report["document_cells"]:
            _statistics(cell, policy, METRICS)
        for cell in report["cells"]:
            _statistics(cell, policy, ("render_ns", "emit_ns"))
        if source_digest() != pinned or harness_sources() != harness_before:
            raise ValueError("source changed during measurement")
        if sha(manifest.read_bytes()) != digest:
            raise ValueError("manifest changed during measurement")
        if any(sha(Path(path).read_bytes()) != expected_hash for identity in identities.values()
               for path, expected_hash in identity.get("artifacts", {}).items()):
            raise ValueError("an engine/module artifact changed during measurement")
        report["gate"] = {"pass": False, "reasons": ["exploratory real pages without independent oracles; insufficient release samples; release matrix incomplete"]}
        atomic_json(output / "conversion.json", report)
        atomic_json(output / "cpu-summary.json", cpu_summary(output))
        measured, drift = {}, {}  # set when retiming
        if retiming:
            measured = compare.medians(output, args.samples)
            drift = compare.control_drift(controls, measured, reference["medians"]) if reference else compare.control_drift([], {}, {})
            result = compare.comparison(report["round5"]["selection"], measured, reference, names)
            result["control"] = drift
            result["comparison_status"] = drift["status"]
            report["retiming"]["comparison_status"] = drift["status"]
            atomic_json(output / "medians.json", list(measured.values()))
            atomic_json(output / "comparison.json", result)
            atomic_json(output / "conversion.json", report)
            summary = compare.markdown(result, drift, reference["provenance"] if reference else None)
            (output / "summary.md").write_text(summary)
            print(summary, flush=True)
        else:
            write_summary(output / "conversion.json", output / "summary.md")
        if "v8-wasi" in names:
            with (output / "summary.md").open("a") as stream:
                stream.write(f"\n{V8_NOTE}\n")
        if args.pagura_x86_64_v3 and identities[X86_V3]["status"] != "available":
            with (output / "summary.md").open("a") as stream:
                stream.write(f"\n| Optional native track | Status | Reason |\n|---|---|---|\n"
                             f"| {X86_V3} | unavailable | {identities[X86_V3]['reason']} |\n")
        exact = all(item["exact"] for item in report["runtime_parity"])
        bad = any(sample[key]["status"] not in GOOD for cell in report["document_cells"]
                  for sample in cell["samples"] + cell["warmup_failures"] for key in ("pagura", "baseline"))
        if retiming:
            bad = any(cell["status"] != "measured" for cell in measured.values())
        status = {"status": "complete", "page_dpi_cases": len(selected), "jobs": len(jobs),
                  "samples": args.samples, "warmups": args.warmups, "runtime_pixel_parity": exact, "failed_cells": bad,
                  "release_eligible": False}
        if retiming:
            status["comparison_status"] = drift["status"]
            if not report["runtime_parity"]:
                status["runtime_pixel_parity"] = None
        atomic_json(output / "status.json", status)

        return result_exit_status(exact, bad, drift["status"] if retiming else None)
    except BaseException as error:
        if report is not None:
            atomic_json(output / "conversion.json", report)
        atomic_json(output / "status.json", {"status": "incomplete", "error": str(error), "release_eligible": False})
        raise


if __name__ == "__main__":
    raise SystemExit(main())
