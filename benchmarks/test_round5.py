"""Independent selection, parity, and accounting failure controls."""
import importlib.util
import json
import random
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from round5 import (CONTENT, V8_FLAGS, X86_V3, build, build_x86_v3, cfg_matches, config_rustflags, cpu_summary,
                    failure_sink, idle_record, measurement, native_simd, parity, remaining_time, sha, subset_300,
                    target_features, verify_runtime_parity, wasm_command)
from machine_record import probe, snapshot
from benchkit.engines import identify


class MatrixContracts(unittest.TestCase):
    def test_explicit_wasm_build_flags_keep_overflow_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            host = "aarch64-apple-darwin"
            commands = []

            def checked(command, log, *, env=None):
                commands.append((list(map(str, command)), env))
                if "cargo" in command:
                    target = command[command.index("--target") + 1]
                    release = output / "build" / target / "release"
                    (release / "deps").mkdir(parents=True)
                    (release / "deps/libpagura-control.rlib").write_bytes(b"library")
                    (release / "pagura-host.wasm").write_bytes(b"host")

            with patch("round5.subprocess.check_output", return_value=f"host: {host}\n"), \
                    patch("round5.target_features", return_value=[]), patch("round5.checked", side_effect=checked):
                build(output, "wasmtime")
            cargo = [(command, env) for command, env in commands if "cargo" in command]
            self.assertEqual(len(cargo), 2)
            native, wasm = cargo
            self.assertEqual(native[1]["CARGO_ENCODED_RUSTFLAGS"], "")
            self.assertEqual(wasm[1]["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"),
                             ["-C", "target-feature=+simd128", "-C", "overflow-checks=on"])
            adapter = next(command for command, _ in commands
                           if "rustc" in command and "wasm32-wasip1" in command)
            self.assertIn("overflow-checks=on", adapter)
            compiler = json.loads((output / "compiler.json").read_text())
            self.assertEqual(compiler["wasm_rustflags"],
                             ["-C", "target-feature=+simd128", "-C", "overflow-checks=on"])

    def test_v8_compiles_the_whole_module_before_stage_timers(self):
        page = {"path": "/input/a.pdf", "dpi": 72, "page": 0}
        command = wasm_command("v8-wasi", "node", "wasmtime", Path("/artifacts"), [page], Path("/output"))
        self.assertEqual(command[1:1 + len(V8_FLAGS)], list(V8_FLAGS))
        self.assertIn("--no-wasm-lazy-compilation", V8_FLAGS)
        self.assertIn("--no-liftoff", V8_FLAGS)

    def test_coregraphics_is_only_selected_on_macos(self):
        for system in ("darwin", "linux"):
            with self.subTest(platform=system), patch.object(sys, "platform", system):
                names = runpy.run_path(str(Path(__file__).with_name("round5.py")))["NATIVE"]
            expected = ("pagura", "pdfium", "mupdf", "quartz", "hayro", "pdfjs", "poppler")
            self.assertEqual(names, expected if system == "darwin" else tuple(name for name in expected if name != "quartz"))

    def test_native_simd_uses_compiler_features(self):
        baseline = native_simd("x86_64-unknown-linux-gnu", ["sse", "sse2"])
        self.assertIn("SSE2 enabled", baseline)
        self.assertIn("SSE4.1 disabled", baseline)
        self.assertIn("SSE4.1 enabled", native_simd("x86_64-unknown-linux-gnu", ["sse2", "sse4.1", "sse4.2"]))
        self.assertIn("AVX2 disabled", baseline)
        self.assertIn("AVX-512 disabled", baseline)
        tuned = native_simd("x86_64-unknown-linux-gnu", ["sse2", "avx2", "avx512f"])
        self.assertIn("AVX2 enabled", tuned)
        self.assertIn("AVX-512 enabled", tuned)
        for host in ("aarch64-apple-darwin", "aarch64-unknown-linux-gnu"):
            self.assertIn("NEON enabled", native_simd(host, ["neon"]))
        self.assertIn("NEON disabled", native_simd("aarch64-unknown-linux-gnu", []))

    def test_poppler_identity_records_runner_version_from_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "pdftoppm"
            executable.write_bytes(b"version probe control")
            for version in ("24.02.0", "26.09.0"):
                with self.subTest(version=version), patch("benchkit.engines.subprocess.run",
                        return_value=subprocess.CompletedProcess([], 0, "", f"pdftoppm version {version}\n")) as run:
                    identity = identify("poppler", [str(executable)])
                self.assertEqual(identity["status"], "available")
                self.assertEqual(identity["version"], f"pdftoppm version {version}")
                self.assertIn(str(executable), identity["artifacts"])
                self.assertEqual(run.call_args.args[0], [str(executable), "-v"])

    def test_idle_check_excludes_harness_and_probe_tree(self):
        processes = "103 102 100.0 ps\n100 99 100.0 python\n102 100 100.0 sh\n101 99 24.9 other\n"
        def stub(command):
            return {"status": "available", "stdout": processes} if command[0] == "ps" else {"status": "unavailable"}
        with patch("machine_record.probe", side_effect=stub), patch("machine_record.os.getpid", return_value=100):
            record = snapshot()
        self.assertEqual(record["heavy_processes"], [])
        with tempfile.TemporaryDirectory() as directory, patch("round5.snapshot", return_value=record), \
             patch("round5.time.sleep") as sleep:
            machine = {"observations": [], "pauses": []}
            idle_record(machine, Path(directory) / "machine.json", "node", smoke=False)
        sleep.assert_not_called()
        self.assertEqual(machine["pauses"], [])

    def test_process_tree_exclusion_keeps_unrelated_heavy_processes(self):
        processes = "100 99 100.0 python\n101 99 25.0 render\n102 100 100.0 ps\n"
        def stub(command):
            return {"status": "available", "stdout": processes} if command[0] == "ps" else {"status": "unavailable"}
        with patch("machine_record.probe", side_effect=stub), patch("machine_record.os.getpid", return_value=100):
            record = snapshot()
        self.assertEqual(record["heavy_processes"], [{"pid": 101, "parent": 99, "cpu_percent": 25.0, "command": "render"}])

    def test_x86_variant_is_unavailable_without_building_on_arm(self):
        with patch("round5.checked") as build, patch("round5.target_features") as features:
            track = build_x86_v3(Path("run"), ["rustup", "run", "1.99.1"], "aarch64-unknown-linux-gnu", {})
        self.assertEqual(track["status"], "unavailable")
        self.assertIn("x86_64 host", track["reason"])
        build.assert_not_called()
        features.assert_not_called()

    def test_x86_variant_refuses_hosts_missing_required_instructions(self):
        with patch("round5.target_features", side_effect=[["avx2", "sse2"], ["sse2"]]), patch("round5.checked") as build:
            track = build_x86_v3(Path("run"), [], "x86_64-unknown-linux-gnu", {})
        self.assertEqual(track["status"], "unavailable")
        self.assertIn("avx2", track["reason"])
        build.assert_not_called()

    def test_x86_variant_isolated_target_flags_and_feature_record(self):
        env = {"CARGO_TARGET_DIR": "portable", "CARGO_ENCODED_RUSTFLAGS": "", "PATH": "bin"}
        host = "x86_64-unknown-linux-gnu"
        features = ["avx2", "fma", "sse2"]
        with patch("round5.target_features", return_value=features), patch("round5.checked") as build:
            track = build_x86_v3(Path("run"), ["rustup", "run", "1.99.1"], host, env)
        self.assertEqual(track["status"], "available")
        self.assertEqual(track["target_features"], features)
        self.assertEqual(track["binary"], "run/build-x86-64-v3/x86_64-unknown-linux-gnu/release/pagura")
        command = build.call_args.args[0]
        self.assertIn("--offline", command)
        self.assertIn("bundled-data", command)
        variant_env = build.call_args.kwargs["env"]
        self.assertEqual(variant_env["RUSTFLAGS"], "-C target-cpu=x86-64-v3")
        self.assertEqual(variant_env["CARGO_TARGET_DIR"], "run/build-x86-64-v3")
        self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", variant_env)
        self.assertEqual(env["CARGO_TARGET_DIR"], "portable")

    def test_cfg_keys_evaluate_like_cargo(self):
        x86 = ['target_arch="x86_64"', 'target_os="linux"', "unix"]
        self.assertTrue(cfg_matches('cfg(target_arch = "x86_64")', x86))
        self.assertFalse(cfg_matches('cfg(target_arch = "aarch64")', x86))
        self.assertTrue(cfg_matches('cfg(all(unix, target_os = "linux"))', x86))
        self.assertTrue(cfg_matches('cfg(any(windows, target_arch = "x86_64"))', x86))
        self.assertFalse(cfg_matches("cfg(not(unix))", x86))
        for unsupported in ("cfg(target_arch = )", "cfg(not(unix, windows))", 'cfg(unix) cfg(windows)', "cfg(all(unix"):
            with self.subTest(key=unsupported), self.assertRaises(ValueError):
                cfg_matches(unsupported, x86)

    def test_repository_flags_follow_cargo_target_tables(self):
        config = {"target": {"wasm32-wasip1": {"rustflags": ["-C", "target-feature=+simd128", "-C", "overflow-checks=on"]},
                             'cfg(target_arch = "x86_64")': {"rustflags": ["-C", "target-cpu=x86-64-v2"]}},
                  "build": {"rustflags": "-C debuginfo=1"}}
        self.assertEqual(config_rustflags(config, "x86_64-unknown-linux-gnu", ['target_arch="x86_64"']),
                         ["-C", "target-cpu=x86-64-v2"])
        self.assertEqual(config_rustflags(config, "wasm32-wasip1", ['target_arch="wasm32"']),
                         ["-C", "target-feature=+simd128", "-C", "overflow-checks=on"])
        self.assertEqual(config_rustflags(config, "aarch64-apple-darwin", ['target_arch="aarch64"']), ["-C", "debuginfo=1"])
        self.assertEqual(config_rustflags({"target": {}}, "aarch64-apple-darwin", []), [])

    def test_build_passes_repository_flags_explicitly_and_records_them(self):
        host = "x86_64-unknown-linux-gnu"
        resolved = {host: ["-C", "target-cpu=x86-64-v2"], "wasm32-wasip1": ["-C", "target-feature=+simd128"]}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for triple, library in ((host, "libpagura-1.rlib"), ("wasm32-wasip1", "libpagura-2.rlib")):
                deps = output / "build" / triple / "release/deps"
                deps.mkdir(parents=True)
                (deps / library).write_bytes(b"")
            (output / "build/wasm32-wasip1/release/pagura-host.wasm").write_bytes(b"\0asm")
            with patch.dict("os.environ", {"RUSTFLAGS": "-C target-cpu=native"}), \
                 patch("round5.subprocess.check_output", return_value=f"rustc 1.99.1\nhost: {host}\n"), \
                 patch("round5.repository_rustflags", side_effect=lambda toolchain, triple, env: resolved[triple]), \
                 patch("round5.target_features", return_value=["sse2", "sse4.1"]) as features, \
                 patch("round5.checked") as checked, patch("round5.atomic_json") as record:
                build(output, "wasmtime")
        commands = [(call.args[0], call.kwargs.get("env") or {}) for call in checked.call_args_list]
        cargo = [(command, env) for command, env in commands if "cargo" in command]
        self.assertEqual([env["CARGO_ENCODED_RUSTFLAGS"] for _, env in cargo],
                         ["-C\x1ftarget-cpu=x86-64-v2", "-C\x1ftarget-feature=+simd128"])
        self.assertTrue(all("RUSTFLAGS" not in env for _, env in cargo))
        adapters = [command for command, _ in commands if "--extern" in command]
        self.assertEqual([command[-2:] for command in adapters],
                         [["-C", "target-cpu=x86-64-v2"], ["-C", "target-feature=+simd128"]])
        self.assertEqual(features.call_args_list[0].args[3:], ("-C", "target-cpu=x86-64-v2"))
        compiler = record.call_args.args[1]
        self.assertEqual(compiler["native_rustflags"], ["-C", "target-cpu=x86-64-v2"])
        self.assertEqual(compiler["wasm_rustflags"], ["-C", "target-feature=+simd128"])

    def test_target_features_records_only_enabled_cfg_features(self):
        with patch("round5.subprocess.check_output", return_value='debug_assertions\ntarget_feature="sse2"\ntarget_arch="x86_64"\ntarget_feature="avx2"\n'):
            self.assertEqual(target_features([], "x86_64-unknown-linux-gnu", {}), ["avx2", "sse2"])

    def test_optional_row_uses_convert_and_fails_closed_on_pixel_mismatch(self):
        from types import SimpleNamespace
        for mismatch in (False, True):
            with self.subTest(mismatch=mismatch), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "input.pdf"
                source.write_bytes(b"independent input")
                case = {"id": "one", "path": str(source), "page": 0, "dpi": 72,
                        "sha256": sha(source.read_bytes())}
                report = {"cells": [], "document_cells": [], "runtime_parity": [], "agreement": []}
                bases = {"pagura": ["portable"], X86_V3: ["v3"]}
                args = SimpleNamespace(seed=17, samples=1, warmups=0, smoke=True, deadline=float("inf"),
                                       engines=("pagura",), main_jobs={("one", 72)}, control_jobs=set())
                commands = []

                def run(command, *unused, **kwargs):
                    commands.append(command)
                    return {"status": "unverified", "pages": [{"page": 0, "status": "unverified",
                            "width": 1, "height": 1,
                            "pixels_sha256": "changed" if mismatch and command[0] == "v3" else "same"}]}

                with patch("round5.idle_record"), patch("round5.expected_pages", return_value={0: None}), \
                     patch("round5.measure_document", side_effect=run):
                    measurement(report, [[case]], bases, "node", "wasmtime", root,
                                {"timeout_seconds": 10}, root, {}, args)
                self.assertIn(["v3", "convert", "--timings", str(source), str(root / "temporary" / X86_V3),
                               "72", "0", "0"], commands)
                self.assertEqual(sorted(command[0] for command in commands), ["portable", "v3"])
                cell = next(cell for cell in report["document_cells"] if cell["engine"] == X86_V3)
                sample = cell["samples"][0]
                self.assertEqual(sample["pagura"]["status"], "unverified")
                self.assertEqual(sample["baseline"]["status"], "runtime-parity-mismatch" if mismatch else "unverified")
                self.assertEqual(next(item for item in report["runtime_parity"] if item["engine"] == X86_V3)["exact"],
                                 not mismatch)

    def candidates(self):
        cases, metadata, geometry = [], {}, {}
        for content in CONTENT:
            sizes = (["small", "small", "large"] + ["standard"] * 7 if content == "vector-heavy" else
                     ["small", "large", "standard", "standard", "standard"])
            for index, size in enumerate(sizes):
                identifier = f"{content}-{index}"
                for page in range(1 if content == "vector-heavy" else 2):
                    cases.append({"id": identifier, "page": page, "dpi": 72})
                    metadata[identifier, page] = {"content": content, "size": size, "producer": identifier}
                    geometry[identifier, page, 300] = (6000, 6000) if content == "vector-heavy" and size == "large" else (1000, 1000)
        return cases, metadata, geometry

    def test_300_selection_preserves_strata_and_diversity(self):
        cases, metadata, geometry = self.candidates()
        chosen = subset_300(cases, metadata, geometry, 17)
        self.assertEqual(len(chosen), 20)
        self.assertEqual(len({case["id"] for case in chosen}), 20)
        self.assertTrue(all(case["dpi"] == 300 for case in chosen))
        from collections import Counter
        selected = [metadata[case["id"], case["page"]] for case in chosen]
        self.assertEqual(Counter(item["content"] for item in selected), {name: 5 for name in CONTENT})
        self.assertEqual(Counter(item["size"] for item in selected), {"small": 4, "standard": 13, "large": 3})
        self.assertEqual(chosen, subset_300(cases, metadata, geometry, 17))

    def test_missing_stratum_is_not_silently_replaced(self):
        cases, metadata, geometry = self.candidates()
        cases = [case for case in cases if metadata[case["id"], case["page"]]["size"] != "large"]
        with self.assertRaisesRegex(ValueError, "cannot satisfy"):
            subset_300(cases, metadata, geometry, 17)

    def test_parity_requires_success_dimensions_and_exact_pixels(self):
        native = {"status": "unverified", "width": 2, "height": 3, "pixels_sha256": "a" * 64}
        self.assertTrue(parity(native, dict(native)))
        for change in ({"width": 3}, {"pixels_sha256": "b" * 64}, {"status": "failed"}):
            self.assertFalse(parity(native, {**native, **change}))

    def test_denied_machine_probe_is_explicit(self):
        with patch("machine_record.subprocess.run", side_effect=PermissionError("denied")):
            result = probe(["ps"])
        self.assertEqual(result["status"], "unavailable")
        self.assertIn("denied", result["diagnostic"])

    def test_runtime_parity_does_not_mask_an_engine_failure(self):
        good = {"status": "unverified", "width": 1, "height": 1, "pixels_sha256": "a" * 64}
        failed = {"status": "unsupported", "feature": "test.feature", "diagnostic": "precise error"}
        self.assertFalse(verify_runtime_parity(good, failed))
        self.assertEqual(failed["status"], "unsupported")
        changed = {**good, "pixels_sha256": "b" * 64}
        self.assertFalse(verify_runtime_parity(good, changed))
        self.assertEqual(changed["status"], "runtime-parity-mismatch")

    def test_deadline_stops_before_another_process(self):
        from types import SimpleNamespace
        with patch("round5.time.monotonic", return_value=10):
            self.assertEqual(remaining_time(SimpleNamespace(deadline=11)), 1)
            with self.assertRaisesRegex(RuntimeError, "time box exhausted"):
                remaining_time(SimpleNamespace(deadline=10))

    def test_failure_images_stay_inside_run_and_obey_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "bad.ppm"
            source.write_bytes(b"bad")
            sink = failure_sink(root / "run/failures")
            (root / "run").mkdir()
            result = {"status": "invalid-output"}
            sink(source, result, {"maximum_output_bytes": 3}, "untrusted/path")
            artifact = Path(result["artifact"])
            self.assertTrue(artifact.is_relative_to(root / "run"))
            self.assertEqual(artifact.read_bytes(), b"bad")
            source.write_bytes(b"oversize")
            refused = {"status": "invalid-output"}
            sink(source, refused, {"maximum_output_bytes": 3}, "other")
            self.assertNotIn("artifact", refused)
            self.assertEqual(len(list((root / "run/failures").iterdir())), 1)

    def test_failed_accounting_remains_visible_in_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            block = {"pair": 0, "runs": {"test": {"status": "failed", "cpu_ns": None, "peak_rss_bytes": None}}}
            (path / "blocks.jsonl").write_text(json.dumps(block) + "\n")
            result = cpu_summary(path)["test"]
        self.assertEqual(result["samples"], 1)
        self.assertEqual(result["accounted_samples"], 0)
        self.assertIsNone(result["median_cpu_wall"])
        self.assertEqual(result["statuses"], {"failed": 1})

    def test_warmup_parallelism_is_not_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            blocks = [{"pair": pair, "runs": {"test": {"status": "unverified", "cpu_wall_ratio": ratio,
                       "cpu_parallelism_flag": ratio > 1.05, "peak_rss_bytes": 100}}}
                      for pair, ratio in ((-1, 1.2), (0, 0.8))]
            (path / "blocks.jsonl").write_text("\n".join(map(json.dumps, blocks)) + "\n")
            result = cpu_summary(path)["test"]
        self.assertEqual(result["samples_above_1_05"], 0)
        self.assertEqual(result["warmup_samples_above_1_05"], 1)
        self.assertEqual(result["max_cpu_wall_including_warmups"], 1.2)

    @unittest.skipUnless(importlib.util.find_spec("PIL"), "Pillow only installed in isolated competitor environment")
    def test_fast_metrics_match_independent_scalar_reader(self):
        from agreement import pixel_difference
        from benchkit.images import pixel_difference as scalar
        rng = random.Random(17)
        for width, height in ((1, 1), (7, 11), (31, 5)):
            first = (width, height, bytes(rng.randrange(256) for _ in range(width * height * 3)))
            second = (width, height, bytes(rng.randrange(256) for _ in range(width * height * 3)))
            self.assertEqual(pixel_difference(first, second), scalar(first, second))
            self.assertEqual(pixel_difference(first, first), scalar(first, first))
        self.assertEqual(pixel_difference((1, 1, b"\0\0\0"), (2, 1, b"\0" * 6)),
                         scalar((1, 1, b"\0\0\0"), (2, 1, b"\0" * 6)))


if __name__ == "__main__":
    unittest.main()
