#!/usr/bin/env python3
"""Build and check four retained-list image references on native/SIMD/scalar.

Validation only: one output per platform/case/mode; no timing or memory claim.
The coordinator owns the final runner/observer receipt and freezes this driver.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys

sys.dont_write_bytecode = True
CASES = tuple("paint/image-direct-" + name for name in
              ("binary-halves", "gray-quarter-turn", "rgb-fractional", "bilinear"))
MODES = ("exact", "competitive")
PLATFORMS = ("native", "wasi-simd", "wasi-scalar")
MAX_BYTES, MAX_RETAINED, LOG_BYTES = 64 << 20, 512 << 20, 4 << 20


def require(value, message):
    if not value:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def expected_rgba(oracle):
    header = b"P6\n32 32\n255\n"
    require(oracle.startswith(header) and len(oracle) == len(header) + 32 * 32 * 3,
            "unexpected frozen P6 dimensions/payload")
    rgb = oracle[len(header):]
    return b"RGBA8 32 32 premultiplied\n" + b"".join(
        rgb[at:at + 3] + b"\xff" for at in range(0, len(rgb), 3))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "output", "cargo", "node"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    require(all(getattr(args, name).is_absolute() for name in ("root", "output", "cargo", "node")),
            "all command paths must be absolute")
    root, output = args.root.resolve(), args.output.resolve()
    require(root == Path.cwd().resolve(), "run from the declared checkout")
    require(os.environ.get("PAGURA_RUN_ID") and os.environ.get("PAGURA_RUN_RECEIPT"),
            "run through the serial coordinator with a final receipt path")
    forbidden = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET",
                 "CARGO_TARGET_DIR", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN",
                 "NODE_OPTIONS", "NODE_V8_COVERAGE")
    require(not any(os.environ.get(name) for name in forbidden)
            and not any(value for name, value in os.environ.items()
                        if name.startswith("CARGO_PROFILE_") or name.startswith("CARGO_TARGET_")),
            "unexpected build/runtime override")
    os.environ.update(CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1", PYTHONDONTWRITEBYTECODE="1")
    sys.path.insert(0, str(root / "benchmarks"))
    import color_reuse_fixtures as fixtures
    from paired_render import ROOT, source_identity
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    from benchkit.measurement import run_process
    require(ROOT == root, "loaded helpers belong to another checkout")
    wrapper_source = root / "benchmarks/results/sources/n16-output-u16/wasm-diagnostic-src/run_profile_wasi.mjs"
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema": 1, "validation": "main-reconciliation-image-parity", "status": "incomplete",
              "admission": False, "expected_outputs": 24, "builds": [], "samples": [], "failures": [],
              "parity": [], "artifacts": {}, "source_before": source_identity(root),
              "run_id": os.environ["PAGURA_RUN_ID"], "receipt": os.environ["PAGURA_RUN_RECEIPT"],
              "producer_argv": list(sys.orig_argv),
              "scope": "compile_page plus DisplayList::render; full validation in both modes; page0/72dpi/white",
              "limitations": ["Four frozen simple image scenes only; no direct_image dispatch, JPEG corpus or general PDF proof.",
                  "Strict RGB error0 and alpha255; no tolerance changes or reference regeneration.",
                  "No benchmark, speed/memory inference or release admission; raw process/stage counters are retained only.",
                  "Final outer receipt/observer authentication is coordinator-owned; a running snapshot is not terminal proof."]}
    stored = 0

    def retain(relative, raw):
        nonlocal stored
        require(stored + len(raw) <= MAX_RETAINED, "evidence byte budget exceeded")
        path = output / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as handle:
            handle.write(raw)
        report["artifacts"][relative] = {"sha256": sha(raw), "bytes": len(raw)}
        stored += len(raw)
        return path

    def record(path):
        nonlocal stored
        raw = read_bounded_regular(path, MAX_BYTES)
        relative = str(path.relative_to(output))
        require(relative not in report["artifacts"], "duplicate artifact record")
        require(stored + len(raw) <= MAX_RETAINED, "evidence byte budget exceeded")
        report["artifacts"][relative] = {"sha256": sha(raw), "bytes": len(raw)}
        stored += len(raw)
        return raw

    def checkpoint():
        report["stored_bytes"] = stored
        (output / "report.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")

    def invoke(command, timeout, item, log_name):
        item["command"] = command
        process, log = run_process(command, {"timeout_seconds": timeout, "measure_rss": False}, log_budget=LOG_BYTES)
        item["process"] = process
        retain(log_name, log.encode())
        item["log"] = log_name
        require(len(log.encode()) < LOG_BYTES and "\ufffd" not in log, "truncated/non-UTF8 process log")
        return process.get("exit_code") == 0 and process.get("status") != "timeout", log

    try:
        paths = {Path(__file__).resolve(), wrapper_source, root / ".cargo/config.toml",
                 fixtures.MANIFEST, fixtures.REFERENCES, args.cargo.resolve(), args.node.resolve(),
                 Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve()}
        paths.update(Path(module.__file__).resolve() for module in tuple(sys.modules.values())
                     if getattr(module, "__file__", None) and Path(module.__file__).resolve().is_relative_to(root))
        pins = {str(path): hash_regular_file(path) for path in sorted(paths)}
        report["bound_files_sha256"] = pins
        report["runtime"] = {"python": sys.version, "sys_executable": str(Path(sys.executable).resolve()),
                             "actual_python_argv0": str(Path(sys.orig_argv[0]).resolve()),
                             "cargo_requested": str(args.cargo), "cargo_resolved": str(args.cargo.resolve()),
                             "node_requested": str(args.node), "node_resolved": str(args.node.resolve())}
        report["environment"] = {name: os.environ.get(name) for name in (*forbidden, "CARGO_BUILD_JOBS", "RUST_TEST_THREADS")}
        retain("runner-start.json", read_bounded_regular(Path(report["receipt"]), MAX_BYTES))
        retain("references/manifest.json", read_bounded_regular(fixtures.MANIFEST, MAX_BYTES))
        retain("references/references.json", read_bounded_regular(fixtures.REFERENCES, MAX_BYTES))
        wrapper = retain("code/run_profile_wasi.mjs", read_bounded_regular(wrapper_source, MAX_BYTES))
        retain("code/parity.py", read_bounded_regular(Path(__file__).resolve(), MAX_BYTES))
        inputs, expected = {}, {}
        for name in CASES:
            pdf, oracle = fixtures.fixture(name)
            inputs[name] = retain("inputs/" + name + ".pdf", pdf)
            retain("oracles/" + name + ".ppm", oracle)
            expected[name] = expected_rgba(oracle)
        report["plan"] = {"cases": list(CASES), "modes": list(MODES), "platforms": list(PLATFORMS),
                          "outputs": 24, "operation": "full", "dpi": 72, "max_channel_error": 0,
                          "alpha": 255, "entrypoint": "compile_page+DisplayList::render", "validation": "full"}
        retain("plan.json", (json.dumps({"plan": report["plan"], "source": report["source_before"],
               "bound_files_sha256": pins, "runtime": report["runtime"]}, indent=2) + "\n").encode())
        binaries = {}
        for platform in PLATFORMS:
            target = root / ("target" if platform == "native" else "target/main-reconciliation-" + platform)
            rustflags = None if platform == "native" else "-C target-feature=" + ("+simd128" if platform == "wasi-simd" else "-simd128")
            os.environ.pop("CARGO_ENCODED_RUSTFLAGS", None)
            if rustflags is None:
                os.environ.pop("RUSTFLAGS", None)
            else:
                os.environ["RUSTFLAGS"] = rustflags
            command = [str(args.cargo), "build", "--locked", "--offline", "--release", "-p", "pagura-render",
                       "--example", "scene_index_profile", "--target-dir", str(target), "--message-format=json", "-j", "1"]
            if platform != "native":
                command += ["--target", "wasm32-wasip1"]
            item = {"platform": platform, "rustflags": rustflags, "cargo_encoded_rustflags": None}
            report["builds"].append(item)
            ok, log = invoke(command, 900, item, "builds/" + platform + ".log")
            require(ok, platform + " build failed")
            artifacts = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
            artifacts = [item for item in artifacts if item.get("reason") == "compiler-artifact"
                         and item.get("target", {}).get("name") == "scene_index_profile" and item.get("executable")]
            require(len(artifacts) == 1, "ambiguous Cargo example artifact")
            compiled = target / ("release/examples/scene_index_profile" if platform == "native" else
                                  "wasm32-wasip1/release/examples/scene_index_profile.wasm")
            require(Path(artifacts[0]["executable"]).resolve() == compiled, "unexpected Cargo artifact path")
            item["cargo_artifact"] = artifacts[0]
            binary = retain("modules/" + platform + ("" if platform == "native" else ".wasm"),
                            read_bounded_regular(compiled, MAX_BYTES))
            binary.chmod(0o500 if platform == "native" else 0o400)
            binaries[platform] = binary
            item["retained_binary"] = str(binary.relative_to(output))
            item["source_after"] = source_identity(root)
            require(item["source_after"] == report["source_before"], "source changed during build")
            checkpoint()
        os.environ.pop("RUSTFLAGS", None)
        for name in CASES:
            for mode in MODES:
                same_mode = []
                for platform in PLATFORMS:
                    folder = output / "samples" / name / mode / platform
                    folder.mkdir(parents=True)
                    path = folder / "output.rgba"
                    item = {"case": name, "mode": mode, "platform": platform, "status": "failed"}
                    report["samples"].append(item)
                    for source in (binaries[platform], inputs[name], wrapper):
                        require(hash_regular_file(source) == report["artifacts"][str(source.relative_to(output))]["sha256"],
                                "retained execution input changed before launch")
                    if platform == "native":
                        command = [str(binaries[platform]), str(inputs[name]), mode, "full", str(path), "72"]
                    else:
                        command = [str(args.node), str(wrapper), str(binaries[platform]), str(inputs[name]), mode,
                                   str(path), str(folder / "wasi.stdout"), str(folder / "wasi.stderr"), str(folder / "wrapper.json")]
                    ok, log = invoke(command, 120, item, str(folder.relative_to(output) / "process.log"))
                    if platform != "native":
                        for filename in ("empty.stdin", "wasi.stdout", "wasi.stderr", "wrapper.json"):
                            if (folder / filename).exists():
                                record(folder / filename)
                        if ok:
                            metadata = json.loads(read_bounded_regular(folder / "wrapper.json", MAX_BYTES))
                            require(metadata["status"] == "completed" and metadata["exit_code"] == 0
                                    and metadata["module_sha256"] == report["artifacts"][str(binaries[platform].relative_to(output))]["sha256"]
                                    and metadata["input_sha256"] == report["artifacts"][str(inputs[name].relative_to(output))]["sha256"]
                                    and metadata["wrapper_sha256"] == report["artifacts"][str(wrapper.relative_to(output))]["sha256"]
                                    and metadata["runtime"]["executable"] == str(args.node.resolve())
                                    and metadata["argv"] == [str(args.node.resolve()), *command[1:]]
                                    and metadata["runtime"]["exec_argv"] == [] and metadata["wasi_env"] == {}
                                    and metadata["wasi_args"] == ["scene_index_profile", "/input/" + inputs[name].name,
                                        mode, "full", "/output/output.rgba", "72"]
                                    and metadata["preopens"] == {"/input": str(inputs[name].parent), "/output": str(folder)},
                                    "WASI invocation or artifact binding differs")
                            log = read_bounded_regular(folder / "wasi.stdout", MAX_BYTES).decode("utf-8")
                    raw = record(path) if path.exists() else None
                    if raw is not None:
                        item["output_sha256"] = sha(raw)
                        same_mode.append(sha(raw))
                    if ok and raw is not None:
                        stages = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
                        require(len(stages) == 1, "missing/ambiguous stage identity")
                        stage = stages[0]
                        require(stage["schema"] == 1 and stage["accuracy"] == mode and stage["operation"] == "full"
                                and stage["dpi"] == 72 and stage["width"] == stage["height"] == 32,
                                "wrong rendered request")
                        item["stages"] = stage
                        item["rgb_and_opaque_alpha_equal"] = raw == expected[name]
                        if raw == expected[name]:
                            item["status"] = "passed"
                    if item["status"] != "passed":
                        report["failures"].append({"case": name, "mode": mode, "platform": platform,
                                                   "reason": "process/output/strict independent pixels failed"})
                    checkpoint()
                equal = len(same_mode) == 3 and len(set(same_mode)) == 1
                report["parity"].append({"case": name, "mode": mode, "equal": equal, "sha256": same_mode})
        report["source_after"] = source_identity(root)
        require(report["source_before"] == report["source_after"], "source changed during validation")
        require(all(hash_regular_file(Path(path)) == digest for path, digest in pins.items()), "bound source/runtime changed")
        require(all(hash_regular_file(output / path) == pin["sha256"] for path, pin in report["artifacts"].items()),
                "retained artifact changed")
        require(len(report["samples"]) == 24 and len(report["parity"]) == 8, "incomplete output matrix")
        report["status"] = "passed-all-24" if not report["failures"] and all(p["equal"] for p in report["parity"]) else "failed-quality"
    except BaseException as error:
        report.update(status="invalid-or-incomplete", fatal_error=f"{type(error).__name__}: {error}")
        raise
    finally:
        os.environ.pop("RUSTFLAGS", None)
        checkpoint()
    print(json.dumps({"status": report["status"], "report": str(output / "report.json")}))
    return 0 if report["status"] == "passed-all-24" else 1


if __name__ == "__main__":
    raise SystemExit(main())
