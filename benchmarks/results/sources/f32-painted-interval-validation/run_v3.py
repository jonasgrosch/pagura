#!/usr/bin/env python3
"""Six serial F32-interval renderer test arms; unchanged layout/PI-Q4 diagnostics."""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import struct
import sys

sys.dont_write_bytecode = True
ARMS, PLATFORMS = ("baseline", "candidate"), ("native", "wasi-simd", "wasi-scalar")
TEST_ARGS = ["--nocapture", "--test-threads=1"]
TYPES = ("Buffer", "Option<Buffer>", "Surface", "MaskSink", "Context",
         "Vec<[u16;4]>", "Range<usize>", "[u16;4]")
Q4_COUNTS = {"page": 4, "output": 2, "source": 20, "function": 3, "command": 1,
             "singular": 2, "none-stroke": 2, "depth": 2, "inverse-overflow": 2,
             "f32-function": 1, "region": 6, "translucent-source": 2}
REQUIRED = {"compositor::temporary_n16_layout_probe",
            "compositor::tests::temporary_n16_resource_boundary_probe",
            "compositor::tests::narrow_rectangles_charge_their_region_as_masks",
            "display::tests::opaque_colour_errors_survive_region_rejection_in_both_modes",
            "compositor::translucency_tests::competitive_f32_fragments_keep_offscreen_source_errors"}
PUBLIC = {"tests::painted_interval::integer_rectangles_match_irregular_and_offset_tiles_in_both_modes",
          "tests::painted_interval::context_painted_then_blank_output_refills_recycled_storage_in_both_modes"}
DYNAMIC = {"compositor::dynamic_interval_tests::" + name for name in (
    "dynamic_consumers_mark_full_and_keep_independent_half_alpha_pixels",
    "disjoint_dynamic_operations_leave_the_interval_empty",
    "empty_shading_samples_still_conservatively_mark_the_dynamic_operation",
    "image_area_charge_failure_precedes_dynamic_preparation",
    "late_function_failure_keeps_full_range_partial_pixels_and_charges",
)}
LOG_BYTES, MAX_FILE = 16 << 20, 512 << 20


def require(value, message):
    if not value:
        raise ValueError(message)


def parse_tests(log, arm, target):
    names = re.findall(r"^test ([\w:]+) \.\.\. ", log, re.M)
    summary = re.findall(r"^test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", log, re.M)
    require(len(summary) == 1 and int(summary[0]) == len(names) > 0
            and len(names) == len(set(names)) and REQUIRED <= set(names), "incomplete successful test inventory")
    require(arm != "candidate" or (PUBLIC | DYNAMIC) <= set(names), "candidate interval/dynamic tests did not run")
    layouts, q4 = {}, []
    for line in log.splitlines():
        if "PAGURA_N16_LAYOUT_V1 " in line:
            row = line.split("PAGURA_N16_LAYOUT_V1 ", 1)[1]
            match = re.fullmatch(r"arch=(\S+) os=(\S*) pointer_bits=(\d+) simd128=(true|false) type=(\S+) size=(\d+) align=(\d+)", row)
            require(match is not None, "malformed layout row")
            arch, system, bits, simd, name, size, align = match.groups()
            require(name in TYPES and name not in layouts, "duplicate/unknown layout type")
            expected = (("aarch64" if platform.machine() == "arm64" else platform.machine()),
                        {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}[platform.system()],
                        str(8 * struct.calcsize("P")), "false") if target == "native" else (
                            "wasm32", "", "32", "true" if target == "wasi-simd" else "false")
            require((arch, system, bits, simd) == expected, "layout target/flags differ")
            layouts[name] = {"size": int(size), "align": int(align)}
        if "PAGURA_PI_Q4_V1 " in line:
            q4.append(line.split("PAGURA_PI_Q4_V1 ", 1)[1])
    require(set(layouts) == set(TYPES) and len(q4) == 47, "missing layout/PI-Q4 records")
    require(Counter(row.split()[0].split("=")[0] for row in q4) == Q4_COUNTS, "PI-Q4 record inventory differs")
    return {"tests_passed": len(names), "test_names": names, "layouts": layouts, "pi_q4": q4}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("preparation", "output", "target_dir", "cargo", "node"):
        parser.add_argument("--" + key.replace("_", "-"), required=True, type=Path)
    parser.add_argument("--preparation-sha256", required=True)
    args = parser.parse_args()
    require(all(getattr(args, key).is_absolute() for key in ("preparation", "output", "target_dir", "cargo", "node")), "absolute paths required")
    raw = args.preparation.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == args.preparation_sha256, "preparation pin differs")
    preparation = json.loads(raw)
    roots = {arm: Path(preparation["roots"][arm]["installed"]["root"]).resolve() for arm in ARMS}
    require(roots["candidate"] == Path.cwd().resolve() and roots["baseline"] != roots["candidate"], "wrong calling/arm roots")
    require(os.environ.get("PAGURA_RUN_ID") and os.environ.get("PAGURA_RUN_RECEIPT"), "owned runner receipt required")
    forbidden = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET",
                 "CARGO_TARGET_DIR", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN",
                 "NODE_OPTIONS", "NODE_V8_COVERAGE")
    require(not any(os.environ.get(key) for key in forbidden)
            and not any(value for key, value in os.environ.items() if key.startswith(("CARGO_PROFILE_", "CARGO_TARGET_"))), "unexpected build/runtime override")
    sys.path.insert(0, str(roots["candidate"] / "benchmarks"))
    from paired_render import source_identity
    from benchkit import measurement
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    require(not args.target_dir.exists(), "fresh isolated build root required")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    original_cwd = measurement.ROOT
    old_env = {key: os.environ.get(key) for key in (*forbidden, "CARGO_BUILD_JOBS", "RUST_TEST_THREADS", "PYTHONDONTWRITEBYTECODE")}
    for key in forbidden:
        os.environ.pop(key, None)
    os.environ.update(CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1", PYTHONDONTWRITEBYTECODE="1")
    report = {"schema": 1, "experiment": "f32-interval-layout-pi-q4", "status": "incomplete", "admission": False,
              "producer_argv": list(sys.orig_argv), "run_id": os.environ["PAGURA_RUN_ID"],
              "receipt": os.environ["PAGURA_RUN_RECEIPT"], "preparation_sha256": args.preparation_sha256,
              "commands": [], "observations": [], "artifacts": {}, "expected_builds": 6,
              "expected_test_runs": 6, "limitations": ["Layout and correctness diagnostics only; no speed/RSS/peak-stack claim.",
                  "Pinned Rust reports an empty std::env::consts::OS on WASI; compiler target cfg, Cargo target, module execution and feature rows are checked separately.",
                  "The WASI arm root is a filesystem preopen, not a read-only capability; source/fixture mutation is checked.",
                  "Final receipt/guard and probe removal are coordinator-owned. Cancellation can prevent run_process returning its partial captured log."]}

    def checkpoint():
        temporary = output / "report.tmp"
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        temporary.replace(output / "report.json")

    def retain(name, data):
        path = output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as handle:
            handle.write(data)
        report["artifacts"][name] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
        return path

    def invoke(root, command, name, timeout=1200):
        item = {"cwd": str(root), "command": command, "rustflags": os.environ.get("RUSTFLAGS"), "status": "started"}
        report["commands"].append(item)
        checkpoint()
        measurement.ROOT = root  # run_process's sole cwd seam; no source is changed.
        try:
            process, log = measurement.run_process(command, {"timeout_seconds": timeout, "measure_rss": False}, log_budget=LOG_BYTES)
            item.update(process=process, log=name, status="returned")
            retain(name, log.encode())
            checkpoint()
            require(len(log.encode()) < LOG_BYTES and "\ufffd" not in log, "truncated/non-UTF8 log")
            require(process.get("exit_code") == 0 and process.get("status") != "timeout", "command failed: " + name)
            return log
        except BaseException as error:
            item.update(status="failed", error=f"{type(error).__name__}: {error}")
            if "log" not in item:
                item["log_unavailable"] = "run_process raised before returning its captured log"
            raise
        finally:
            measurement.ROOT = original_cwd
            checkpoint()

    def fixtures(root):
        return {str(path.relative_to(root)): hash_regular_file(path)
                for path in sorted((root / "corpus/generated").rglob("*")) if path.is_file()}

    def cancel(signum, _frame):
        raise measurement.MeasurementCancelled(signum)

    checkpoint()
    handlers = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
    for signum in handlers:
        signal.signal(signum, cancel)
    try:
        report["source_before"] = {arm: source_identity(root) for arm, root in roots.items()}
        require(all(report["source_before"][arm] == preparation["roots"][arm]["installed"] for arm in ARMS), "installed sources changed")
        report["fixtures_before"] = {arm: fixtures(root) for arm, root in roots.items()}
        require(report["fixtures_before"]["baseline"] and report["fixtures_before"]["baseline"] == report["fixtures_before"]["candidate"], "fixture arms differ or empty")
        wrapper_source = Path(__file__).with_name("run_wasi_diagnostic.mjs")
        paths = {Path(__file__).resolve(), wrapper_source.resolve(), args.preparation.resolve(), args.cargo.resolve(),
                 args.node.resolve(), Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve()}
        paths.update(Path(item["path"]) for item in preparation["patches"])
        for item in preparation["patches"]:
            require(hash_regular_file(Path(item["path"])) == item["sha256"], "temporary patch changed")
        paths.update(Path(module.__file__).resolve() for module in tuple(sys.modules.values())
                     if getattr(module, "__file__", None) and Path(module.__file__).resolve().is_relative_to(roots["candidate"]))
        paths.update(root / name for root in roots.values() for name in (".cargo/config.toml", "rust-toolchain.toml"))
        cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
        configs = {cargo_home / name for name in ("config", "config.toml")}
        configs.update(parent / ".cargo" / name for root in roots.values()
                       for parent in (root, *root.parents) for name in ("config", "config.toml"))
        paths.update(path.resolve() for path in configs if path.is_file())
        require(all((roots["baseline"] / name).read_bytes() == (roots["candidate"] / name).read_bytes()
                    for name in (".cargo/config.toml", "rust-toolchain.toml", "Cargo.lock", "Cargo.toml")), "build configurations differ")
        report["runtime"] = {"python": sys.version, "executable": sys.executable, "orig_argv": sys.orig_argv,
                             "cargo_requested": str(args.cargo), "node_requested": str(args.node)}
        report["environment"] = {key: value for key, value in os.environ.items()
                                 if key.startswith(("CARGO_", "RUST", "NODE_")) or key in ("PATH", "CC", "CXX", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET", "PYTHONDONTWRITEBYTECODE")}
        wrapper = retain("code/run_wasi_diagnostic.mjs", read_bounded_regular(wrapper_source, MAX_FILE))
        retain("code/run.py", Path(__file__).read_bytes())
        retain("preparation.json", raw)
        retain("runner-start.json", read_bounded_regular(Path(report["receipt"]), MAX_FILE))
        rustup = args.cargo.parent / "rustup"
        paths.add(rustup.resolve())
        for tool in ("cargo", "rustc"):
            selected = invoke(roots["candidate"], [str(rustup), "which", tool], "runtime/which-" + tool + ".log").strip()
            selected = Path(selected)
            require(selected.is_absolute() and selected.is_file(), "invalid selected toolchain path")
            paths.add(selected.resolve())
            invoke(roots["candidate"], [str(selected), "-vV"], "runtime/" + tool + ".log")
            if tool == "rustc":
                cfg = invoke(roots["candidate"], [str(selected), "--print", "cfg", "--target", "wasm32-wasip1"], "runtime/wasi-cfg.log")
                require('target_arch="wasm32"' in cfg.splitlines() and 'target_os="wasi"' in cfg.splitlines(), "selected WASI compiler target differs")
        invoke(roots["candidate"], [str(args.node), "--version"], "runtime/node.log")
        pins = {str(path): hash_regular_file(path) for path in sorted(paths)}
        report["bound_files_sha256"] = pins
        checkpoint()
        for arm, root in roots.items():
            for target in PLATFORMS:
                flags = None if target == "native" else "-C target-feature=" + ("+simd128" if target == "wasi-simd" else "-simd128")
                os.environ.pop("RUSTFLAGS", None)
                if flags:
                    os.environ["RUSTFLAGS"] = flags
                target_dir = args.target_dir.resolve() / arm / target
                command = [str(args.cargo), "test", "--locked", "--offline", "--release", "-j", "1",
                           "-p", "pagura-render", "--lib", "--no-run", "--message-format=json", "--target-dir", str(target_dir)]
                if target != "native":
                    command += ["--target", "wasm32-wasip1"]
                log = invoke(root, command, f"{arm}/{target}-build.log")
                messages = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
                finished = [item for item in messages if item.get("reason") == "build-finished"]
                artifacts = [item for item in messages if item.get("reason") == "compiler-artifact"
                             and item.get("target", {}).get("name") == "pagura_render"
                             and item["target"].get("kind") == ["lib"] and item.get("profile", {}).get("test") is True
                             and item.get("executable")]
                require(len(finished) == 1 and finished[0].get("success") is True and len(artifacts) == 1, "ambiguous/incomplete Cargo test build")
                source = Path(artifacts[0]["executable"]).resolve()
                require(source.is_relative_to(target_dir), "test artifact outside dedicated target directory")
                binary = retain(f"{arm}/{target}-tests" + (".wasm" if target != "native" else ""), read_bounded_regular(source, MAX_FILE))
                binary.chmod(0o500 if target == "native" else 0o400)
                digest = hash_regular_file(binary)
                require(hash_regular_file(source) == digest, "test artifact changed during archival")
                item = {"arm": arm, "platform": target, "cargo_artifact": artifacts[0], "binary": str(binary), "binary_sha256": digest}
                report["observations"].append(item)
                command = [str(binary), *TEST_ARGS] if target == "native" else [str(args.node), str(wrapper), str(binary), str(root), *TEST_ARGS]
                test_log = invoke(root, command, f"{arm}/{target}-tests.log")
                if target != "native":
                    metadata = [json.loads(line) for line in test_log.splitlines() if line.startswith('{"schema":')]
                    require(len(metadata) == 1 and metadata[0]["module"] == {"path": str(binary), "sha256": digest}
                            and metadata[0]["test_args"] == TEST_ARGS and metadata[0]["preopens"] == {str(root): str(root)}
                            and Path(metadata[0]["runtime"]["executable"]).resolve() == args.node.resolve()
                            and metadata[0]["runtime"]["exec_argv"] == [], "WASI execution binding differs")
                    item["wasi"] = metadata[0]
                item.update(parse_tests(test_log, arm, target))
                checkpoint()
        report["comparisons"] = []
        for target in PLATFORMS:
            pair = {item["arm"]: item for item in report["observations"] if item["platform"] == target}
            require(pair["baseline"]["pi_q4"] == pair["candidate"]["pi_q4"], "same-platform PI-Q4 transcript differs")
            deltas = {name: {key: pair["candidate"]["layouts"][name][key] - pair["baseline"]["layouts"][name][key]
                             for key in ("size", "align")} for name in TYPES}
            report["comparisons"].append({"platform": target, "pi_q4_equal": True, "layout_deltas": deltas,
                                          "no_layout_growth": all(value <= 0 for row in deltas.values() for value in row.values())})
        report["source_after"] = {arm: source_identity(root) for arm, root in roots.items()}
        report["fixtures_after"] = {arm: fixtures(root) for arm, root in roots.items()}
        require(report["source_after"] == report["source_before"] and report["fixtures_after"] == report["fixtures_before"], "source/fixtures changed")
        require(all(hash_regular_file(Path(path)) == digest for path, digest in pins.items()), "bound source/runtime changed")
        require(all(hash_regular_file(output / name) == item["sha256"] for name, item in report["artifacts"].items()), "retained artifact changed")
        require(len(report["observations"]) == 6, "incomplete six-arm matrix")
        report["layout_gate_passed"] = all(item["no_layout_growth"] for item in report["comparisons"])
        report["status"] = "verified-tests-and-diagnostics" if report["layout_gate_passed"] else "blocked-layout-growth"
    except BaseException as error:
        report.update(status="failed-or-incomplete", fatal_error=f"{type(error).__name__}: {error}", layout_gate_passed=False)
        raise
    finally:
        measurement.ROOT = original_cwd
        try:
            if "source_after" not in report:
                report["source_after"] = {arm: source_identity(root) for arm, root in roots.items()}
            if "fixtures_after" not in report:
                report["fixtures_after"] = {arm: fixtures(root) for arm, root in roots.items()}
        except BaseException as error:
            report.update(status="failed-or-incomplete", layout_gate_passed=False,
                          final_identity_error=f"{type(error).__name__}: {error}")
        for key, value in old_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        try:
            checkpoint()
        finally:
            for signum, handler in handlers.items():
                signal.signal(signum, handler)
    print(json.dumps({"status": report["status"], "report": str(output / "report.json")}))
    return 0 if report["status"] == "verified-tests-and-diagnostics" else 2


if __name__ == "__main__":
    raise SystemExit(main())
