"""Protect benchmark validity: empty, wrong, missing and incomparable evidence fails."""
import copy
import unittest
import tempfile
import json
import os
import subprocess
import sys
from unittest.mock import patch
from pathlib import Path

from benchkit.config import read_json, POLICY_PATH, sha
from benchkit.statistics import evaluate_cell, ratio_bounds, comparison_statistics
from benchkit.images import ppm, normalize, pixel_difference
from benchkit.timings import parse_stages, parse_document_stages
from benchkit.gate import gate
from benchkit import corpus, conversion, measurement, reports, documents
from benchkit import engines as engine_tools
import competitors


class ImageTests(unittest.TestCase):
    def test_binary_whitespace_and_hash_bytes_are_pixels(self):
        self.assertEqual(ppm(b"P6\n# note\n1 1\n255\n\n #"), (1, 1, b"\n #"))

    def test_header_normalization_and_crlf(self):
        self.assertEqual(ppm(b"P6 1 1 255\nabc"), ppm(b"P6\r\n1 1\r\n255\r\nabc"))

    def test_truncation_extra_bytes_and_oversized_dimensions_fail(self):
        for data in [b"P6", b"P6 1 1 255\nab", b"P6 1 1 255\nabcd", b"P6 999999 999999 255\na", b"P6 1 1 65535\nabc"]:
            with self.assertRaises(ValueError):
                ppm(data)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.policy = read_json(POLICY_PATH)
        # Fewer resamples for synthetic tests only; production policy stays fixed.
        self.policy["bootstrap_resamples"] = 100
        digest = sha(b"abc")
        self.cell = {"engine": "example", "status": "measured", "oracle_pixels_sha256": digest,
                     "samples": [{"pair": i, "order": ["pagura", "example"],
                                  "pagura": {"status": "ok", "exit_code": 0, "pixels_sha256": digest,
                                             "elapsed_ns": 70, "peak_rss_bytes": 50},
                                  "baseline": {"status": "ok", "exit_code": 0, "pixels_sha256": digest,
                                               "elapsed_ns": 100, "peak_rss_bytes": 100}}
                                 for i in range(self.policy["minimum_samples"])]}

    def test_complete_fast_correct_cell_passes(self):
        self.assertTrue(evaluate_cell(self.cell, self.policy)["pass"])

    def test_missing_failed_incorrect_and_nonfinite_samples_fail(self):
        for key, value in [("status", "timeout"), ("exit_code", 1), ("pixels_sha256", "bad"),
                           ("elapsed_ns", float("nan")), ("elapsed_ns", -1), ("peak_rss_bytes", None)]:
            cell = copy.deepcopy(self.cell)
            cell["samples"][0]["pagura"][key] = value
            self.assertFalse(evaluate_cell(cell, self.policy)["pass"], key)

    def test_a_fast_median_does_not_hide_slow_tail(self):
        cell = copy.deepcopy(self.cell)
        for sample in cell["samples"][-10:]:
            sample["pagura"]["elapsed_ns"] = 200
        verdict = evaluate_cell(cell, self.policy)
        self.assertFalse(verdict["pass"])
        self.assertTrue(any("p95" in reason for reason in verdict["reasons"]))

    def test_memory_regression_fails_even_with_fast_timings(self):
        self.cell["samples"][0]["pagura"]["peak_rss_bytes"] = 101
        self.assertFalse(evaluate_cell(self.cell, self.policy)["pass"])

    def test_missing_or_duplicate_pairs_fail(self):
        for samples in [[], self.cell["samples"][:2], [self.cell["samples"][0]] * 100]:
            cell = {**self.cell, "samples": samples}
            self.assertFalse(evaluate_cell(cell, self.policy)["pass"])

    def test_empty_release_report_cannot_pass(self):
        verdict = gate({"schema": 1, "suite": "release"})
        self.assertFalse(verdict["pass"])
        self.assertTrue(any("empty" in reason for reason in verdict["reasons"]))
        self.assertTrue(any("adobe" in reason for reason in verdict["reasons"]))

    def test_stale_identity_and_incomparable_settings_fail(self):
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder) / "example"
            binary.write_bytes(b"changed")
            report = {"schema": 1, "suite": "release", "settings": {"dpi": 300},
                      "engine_identities": {"pagura": {"status": "available", "version": "test",
                          "artifacts": {str(binary): sha(b"original")}}}}
            reasons = gate(report)["reasons"]
            self.assertIn("stale engine artifact: pagura", reasons)
            self.assertIn("incomparable rendering settings", reasons)

    def test_bootstrap_is_reproducible_and_rejects_zero_samples(self):
        a = ratio_bounds([10, 11, 12], [20, 21, 22], resamples=100)
        self.assertEqual(a, ratio_bounds([10, 11, 12], [20, 21, 22], resamples=100))
        with self.assertRaises(ValueError):
            ratio_bounds([0, 1], [1, 2])


# bench-stage-timers, bench-output-normalization, bench-corpus-verification,
# bench-regression-isolation, bench-release-boundary: independent synthetic bytes
# and clock records below are the oracle, never an engine's expected output.
FAKE_ENGINE = r'''import json
import sys
import time
from pathlib import Path

if sys.argv[1:] == ["--version"]:
    print("independent fake engine 1")
    raise SystemExit
mode = sys.argv[1]
if mode == "timeout":
    time.sleep(10)
if mode == "failure":
    print("deliberate failure", file=sys.stderr)
    raise SystemExit(7)
args = sys.argv[2:]
if args[0] == "render":
    args = args[2:] if args[1] == "--timings" else args[1:]
if mode == "poppler":
    source, output = args[-2:]
    output += ".ppm"
    dpi = args[args.index("-r") + 1]
    page = str(int(args[args.index("-f") + 1]) - 1)
else:
    source, output, dpi, page = args
# RGB bytes 'abc' are manually chosen, not derived from the document or Pagura.
if mode == "wrong":
    pixels = b"abd"
else:
    pixels = b"abc"
if mode == "pam":
    image = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n" + pixels + b"\xff"
elif mode == "rgba":
    image = b"RGBA8 1 1 straight\n" + pixels + b"\xff"
else:
    image = b"P6\n1 1\n255\n" + pixels
if mode != "no-output":
    Path(output).write_bytes(image)
if mode == "oversize":
    Path(output).write_bytes(b"x" * 4096)
if mode == "broken-image":
    Path(output).write_bytes(b"P6\n1 1\n255\na")
if mode not in ("no-timers", "poppler"):
    print(json.dumps({"schema": 1, "open_ns": -1 if mode == "bad-timers" else 100,
        "pages": [{"page": int(page), "render_ns": 200, "emit_ns": 300}], "total_ns": 700}))
'''


class ConversionTests(unittest.TestCase):
    def setUp(self):
        # Darwin `time -l` needs sysctl access that sandboxed test runs may lack.
        # Unit measurements omit it; missing RSS still cannot pass the gate.
        self.rss = patch.object(measurement, "rss_wrapper", return_value=[])
        self.rss.start()
        self.addCleanup(self.rss.stop)
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.fake = self.directory / "engine.py"
        self.fake.write_text(FAKE_ENGINE)
        self.output = self.directory / "result.ppm"
        self.policy = read_json(POLICY_PATH)
        self.policy["bootstrap_resamples"] = 30
        self.source = self.directory / "input.pdf"
        self.source.write_bytes(b"independent synthetic input; not interpreted by fake")
        self.case = {"id": "synthetic", "path": str(self.source), "sha256": sha(self.source.read_bytes()),
                     "page": 0, "dpi": 72}

    def measure(self, mode, expected=(1, 1, b"abc"), **kwargs):
        command = [sys.executable, str(self.fake), mode, str(self.source), str(self.output), "72", "0"]
        return measurement.measure(command, self.output, expected, self.policy, page=0, **kwargs)

    def test_fake_stage_timing_and_each_format(self):
        for mode in ("ppm", "pam", "rgba"):
            result = self.measure(mode, stages_required=True)
            self.assertEqual(result["status"], "ok")
            self.assertEqual(result["stages"], {"open_ns": 100, "render_ns": 200, "emit_ns": 300,
                                                  "engine_ns": 300, "total_ns": 700})
            self.assertEqual(result["pixels_sha256"], sha(b"abc"))
            self.assertGreater(result["elapsed_ns"], 700)

    def test_wrong_missing_and_invalid_outputs_remain_failures(self):
        for mode, status in (("wrong", "incorrect"), ("failure", "failed"), ("no-output", "invalid-output"),
                             ("broken-image", "invalid-output"), ("bad-timers", "invalid-timings")):
            result = self.measure(mode, stages_required=True)
            self.assertEqual(result["status"], status, result)
        self.assertEqual(self.measure("wrong")["max_channel_error"], 1)
        self.policy["maximum_output_bytes"] = 100
        self.assertEqual(self.measure("oversize")["status"], "invalid-output")

    def test_stage_availability_is_explicit_and_required_where_supported(self):
        self.assertEqual(self.measure("no-timers")["stages_status"], "unavailable")
        self.assertEqual(self.measure("no-timers", stages_required=True)["status"], "invalid-timings")

    def test_timeout_and_launch_failure(self):
        self.policy["timeout_seconds"] = 0.05
        self.assertEqual(self.measure("timeout")["status"], "timeout")
        result = measurement.measure([str(self.directory / "missing")], self.output, (1, 1, b"abc"), self.policy)
        self.assertEqual(result["status"], "failed")

    def test_stale_output_cannot_turn_missing_output_into_success(self):
        self.output.write_bytes(b"P6\n1 1\n255\nabc")
        self.assertEqual(self.measure("no-output")["status"], "invalid-output")

    def test_no_oracle_never_becomes_correct_even_when_pixels_agree(self):
        self.assertEqual(self.measure("ppm", expected=None)["status"], "unverified")
        self.assertEqual(pixel_difference((1, 1, b"abc"), (1, 1, b"abd"))["channel_mean_errors"], [0, 0, 1])
        self.assertEqual(pixel_difference((1, 1, b"abc"), (2, 1, b"abcdef"))["status"], "dimension-mismatch")

    def test_multi_engine_report_keeps_unverified_agreement_out_of_gate(self):
        bases = {name: [sys.executable, str(self.fake), mode] for name, mode in
                 (("pagura", "ppm"), ("pdfium", "pam"), ("pdfjs", "rgba"))}
        identities = {name: {"status": "available", "version": "fake"} for name in bases}
        report = conversion.conversion_report([self.case, {**self.case, "page": 1, "dpi": 144}], bases,
                                         identities, self.policy, samples=2, warmups=1, seed=42017,
                                         input_digest=sha(b"manifest"), process_per_page=True)
        self.assertEqual(len(report["cells"]), 4)
        self.assertEqual(len(report["agreement"]), 6)
        for cell in report["cells"]:
            self.assertEqual(cell["verification"], "unverified")
            self.assertFalse(cell["acceptance"]["pass"])
            self.assertEqual(cell["statistics"]["engine_ns"]["median_ratio"], 1)
            self.assertFalse(cell["warmup_failures"])
            self.assertEqual([s["pair"] for s in cell["samples"]], [0, 1])
            for sample in cell["samples"]:
                self.assertEqual(sample["agreement"]["max_channel_error"], 0)
                self.assertEqual(sample["pagura"]["status"], "unverified")
        self.assertFalse(gate(report)["pass"])
        self.assertIn("unverified", reports.conversion_markdown(report))

    def test_report_preserves_failed_pairs_and_prevents_speed_ratio(self):
        oracle = self.directory / "oracle.ppm"
        oracle.write_bytes(b"P6\n1 1\n255\nabc")
        case = {**self.case, "oracle": str(oracle), "oracle_sha256": sha(oracle.read_bytes())}
        bases = {name: [sys.executable, str(self.fake), mode] for name, mode in
                 (("pagura", "ppm"), ("pdfium", "wrong"))}
        report = conversion.conversion_report([case], bases, {name: {"status": "available"} for name in bases},
                                         self.policy, samples=2, warmups=1, seed=1, input_digest=sha(b"manifest"), process_per_page=True)
        cell = report["cells"][0]
        self.assertEqual(len(cell["samples"]), 2)
        self.assertEqual(len(cell["warmup_failures"]), 1)
        self.assertEqual(cell["samples"][0]["baseline"]["status"], "incorrect")
        self.assertEqual(cell["statistics"]["engine_ns"]["status"], "failed-output")
        self.assertFalse(cell["acceptance"]["pass"])
        artifact = Path(cell["samples"][0]["baseline"]["artifact"])
        self.assertEqual(normalize(artifact.read_bytes()), (1, 1, b"abd"))

    def test_verified_conversion_with_fake_rss_can_pass_cell_but_never_release(self):
        oracle = self.directory / "oracle.ppm"
        oracle.write_bytes(b"P6\n1 1\n255\nabc")
        case = {**self.case, "oracle": str(oracle), "oracle_sha256": sha(oracle.read_bytes())}
        wrapper = self.directory / "time.py"
        wrapper.write_text("import subprocess, sys\n" +
                           "code = subprocess.call(sys.argv[1:])\n" +
                           "print('PAGURA_RSS_KIB=100', file=sys.stderr)\n" +
                           "raise SystemExit(code)\n")
        bases = {name: [sys.executable, str(self.fake), "ppm"] for name in ("pagura", "pdfium")}
        policy = {**self.policy, "minimum_samples": 2, "latency_ratio_max": 1000.0}
        # The relaxed threshold is test-local, making the identical-stage positive
        # control independent of noisy process startup. Checked-in policy is unchanged.
        with patch.object(measurement, "rss_wrapper", return_value=[sys.executable, str(wrapper)]), patch.object(measurement.platform, "system", return_value="Linux"):
            report = conversion.conversion_report([case], bases, {name: {"status": "available"} for name in bases},
                policy, samples=2, warmups=1, seed=1, input_digest=sha(b"manifest"), process_per_page=True)
        cell = report["cells"][0]
        self.assertEqual(cell["verification"], "independent-oracle")
        self.assertEqual(cell["samples"][0]["pagura"]["peak_rss_bytes"], 102400)
        self.assertTrue(cell["acceptance"]["pass"], cell["acceptance"])
        self.assertFalse(report["gate"]["pass"])
        self.assertFalse(gate(report)["pass"])

    def test_poppler_process_only_and_missing_engine_cells(self):
        bases = {"pagura": [sys.executable, str(self.fake), "ppm"],
                 "poppler": [sys.executable, str(self.fake), "poppler"], "pdfium": ["/missing"]}
        identities = {name: {"status": "unavailable" if name == "pdfium" else "available"} for name in bases}
        report = conversion.conversion_report([self.case], bases, identities, self.policy,
            samples=2, warmups=0, seed=1, input_digest=sha(b"manifest"), process_per_page=True)
        cells = {cell["engine"]: cell for cell in report["cells"]}
        self.assertEqual(cells["pdfium"]["status"], "unavailable")
        self.assertEqual(cells["pdfium"]["samples"], [])
        self.assertEqual(cells["poppler"]["samples"][0]["baseline"]["stages_status"], "unavailable")
        self.assertEqual(cells["poppler"]["statistics"]["engine_ns"]["status"], "unavailable-timing")
        self.assertEqual(cells["poppler"]["statistics"]["elapsed_ns"]["status"], "measured")

    def test_manifest_hashes_ranges_dpis_and_oracles(self):
        oracle = self.directory / "oracle.ppm"
        oracle.write_bytes(b"P6\n1 1\n255\nabc")
        document = {"id": "real", "path": self.source.name, "sha256": self.case["sha256"], "pages": 3,
                    "oracles": [{"page": 1, "dpi": 72, "path": oracle.name, "sha256": sha(oracle.read_bytes())}]}
        manifest = self.directory / "manifest.json"
        manifest.write_text(json.dumps({"schema": 1, "documents": [document]}))
        cases, digest = corpus.corpus_cases(self.directory, None, "2-3", (72, 144))
        self.assertEqual(digest, sha(manifest.read_bytes()))
        self.assertEqual([(c["page"], c["dpi"]) for c in cases], [(1, 72), (1, 144), (2, 72), (2, 144)])
        self.assertEqual(sum("oracle" in case for case in cases), 1)
        for selection in ("0", "4", "2-1", "x", "1-"):
            with self.assertRaises(ValueError):
                corpus.corpus_cases(self.directory, None, selection)
        with self.assertRaises(ValueError):
            corpus.corpus_cases(self.directory, None, dpis=(0,))
        self.source.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "changed"):
            corpus.corpus_cases(self.directory, None)

    def test_manifest_rejects_escape_duplicate_and_changed_oracle(self):
        document = {"id": "real", "path": "../outside.pdf", "sha256": self.case["sha256"], "pages": 1}
        manifest = self.directory / "manifest.json"
        def save(documents):
            manifest.write_text(json.dumps({"schema": 1, "documents": documents}))
        save([document])
        with self.assertRaises(ValueError):
            corpus.corpus_cases(self.directory, None)
        document["path"] = self.source.name
        save([document, document])
        with self.assertRaisesRegex(ValueError, "unique"):
            corpus.corpus_cases(self.directory, None)
        document["oracles"] = [{"page": 0, "dpi": 72, "path": self.source.name, "sha256": sha(b"wrong")}]
        save([document])
        with self.assertRaisesRegex(ValueError, "changed"):
            corpus.corpus_cases(self.directory, None)

    def test_conversion_commands_select_pages_and_request_real_timers(self):
        case = {**self.case, "page": 3}
        command = engine_tools.conversion_command("pagura", ["pagura"], case, self.output)
        self.assertEqual(command[1:3], ["render", "--timings"])
        self.assertEqual(command[-1], "3")
        command = engine_tools.conversion_command("poppler", ["pdftoppm"], case, self.output)
        self.assertEqual(command[1:6], ["-f", "4", "-l", "4", "-singlefile"])
        self.assertNotIn("--timings", command)

    def test_summary_digests_and_raw_report_location(self):
        root = self.directory / "repo"
        local = root / "benchmarks/results/local"
        local.mkdir(parents=True)
        report = {"operation": "conversion", "input_sha256": sha(b"manifest"), "source_sha256": sha(b"source"),
                  "policy_sha256": sha(b"policy"), "manifest_sha256": sha(b"generated"), "cells": []}
        raw = local / "raw.json"
        raw.write_text(json.dumps(report))
        summary = root / "benchmarks/results/example.md"
        with patch.object(reports, "ROOT", root):
            reports.write_summary(raw, summary)
            self.assertIn(sha(raw.read_bytes()), summary.read_text())
            self.assertIn(report["input_sha256"], summary.read_text())
            with self.assertRaises(FileExistsError):
                reports.write_summary(raw, summary)
            with self.assertRaises(ValueError):
                reports.local_report_path(root / "raw.json")
            self.assertEqual(reports.local_report_path(raw), raw.resolve())
            with self.assertRaises(ValueError):
                reports.write_summary(raw, local / "summary.md")


class FormatAndStageTests(unittest.TestCase):
    def test_rgba_alpha_oracles_are_independent(self):
        self.assertEqual(normalize(b"RGBA8 1 1 straight\n\xff\x00\x00\x80"), (1, 1, b"\xff\x7f\x7f"))
        self.assertEqual(normalize(b"RGBA8 1 1 premultiplied\n\x80\x00\x00\x80"), (1, 1, b"\xff\x7f\x7f"))
        self.assertEqual(normalize(b"RGBA8 1 1 straight\n\x0a #\xff"), (1, 1, b"\x0a #"))

    def test_raw_formats_reject_ambiguous_or_truncated_data(self):
        pam = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\nabc"
        self.assertEqual(normalize(pam), (1, 1, b"abc"))
        for data in (pam[:-1], pam + b"x", pam.replace(b"WIDTH 1", b"WIDTH 1\nWIDTH 1"),
                     pam.replace(b"255", b"65535"), b"RGBA8 1 1 straight\nabc", b"RGBA8 1 1 unknown\nabcd",
                     b"RGBA8 1 1 premultiplied\n\xff\x00\x00\x00", b"RGBA8 99999999 1 straight\n", b"P7\n" * 3000):
            with self.assertRaises(ValueError):
                normalize(data)

    def test_stage_records_fail_closed(self):
        record = {"schema": 1, "open_ns": 10, "pages": [{"page": 2, "render_ns": 20, "emit_ns": 30}], "total_ns": 60}
        text = json.dumps(record)
        self.assertEqual(parse_stages("warning\n" + text, 2, 100)["engine_ns"], 30)
        self.assertIsNone(parse_stages("warning", 2, 100))
        for field, value in (("open_ns", -1), ("open_ns", True), ("open_ns", 1.5), ("open_ns", float("nan")),
                             ("total_ns", 59), ("total_ns", 101), ("schema", 2), ("schema", True), ("pages", [])):
            changed = {**record, field: value}
            with self.assertRaises(ValueError):
                parse_stages(json.dumps(changed), 2, 100)
        for text in ("{broken", '{"open_ns":10,"open_ns":20}', json.dumps(record) + "\n" + json.dumps(record)):
            with self.assertRaises(ValueError):
                parse_stages(text, 2, 100)
        with self.assertRaises(ValueError):
            parse_stages(json.dumps(record), 0, 100)


class IsolationTests(unittest.TestCase):
    def test_cache_outside_repository_and_linked_worktree(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "repo"
            worker = root / "worktrees/feature"
            worker.mkdir(parents=True)
            (worker / ".git").write_text("gitdir: " + str(root / ".git/worktrees/feature"))
            for cache in (root, root / ".cache/competitors", worker / "cache", Path(folder)):
                with patch.dict(os.environ, {"PAGURA_COMPETITORS_DIR": str(cache)}):
                    with self.assertRaises(ValueError):
                        competitors.competitor_cache(worker)
            external = Path(folder) / "isolated"
            with patch.dict(os.environ, {"PAGURA_COMPETITORS_DIR": str(external)}):
                self.assertEqual(competitors.competitor_cache(worker), external.resolve())

    def test_missing_engine_doctor_has_actionable_hint(self):
        result = engine_tools.identify("pdfium", ["/definitely/missing/python"])
        self.assertEqual(result["status"], "unavailable")
        self.assertIn("tools/competitors.py setup", result["hint"])
        self.assertIn(str(competitors.CACHE), result["hint"])

    def test_installer_is_out_of_tree_and_uses_pinned_toolchain(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(competitors, "CACHE", Path(folder)), patch.object(competitors, "run") as run, patch("builtins.print"):
                competitors.setup()
            cargo = next(call for call in run.call_args_list if call.args[0][0] == "cargo")
            self.assertNotIn("+1.97.0", cargo.args[0])
            self.assertEqual(cargo.kwargs["env"]["CARGO_HOME"], str(Path(folder) / "cargo-home"))
            self.assertEqual(cargo.kwargs["cwd"], Path(folder) / "hayro-adapter")
            self.assertEqual((Path(folder) / "hayro-adapter/Cargo.lock").read_bytes(),
                             (engine_tools.ADAPTERS / "hayro/Cargo.lock").read_bytes())

    def test_regression_build_pairing_and_cleanup_in_disposable_repository(self):
        # Actual local Git worktree lifecycle, fake offline Cargo/engine: no product
        # metadata mutation and no compiler download or competitor execution.
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            repo = directory / "repo"
            repo.mkdir()
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / "marker").write_text("revision source")
            subprocess.run(["git", "add", "marker"], cwd=repo, check=True)
            subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@invalid", "commit", "-qm", "fixture"], cwd=repo, check=True)
            shim = directory / "bin"
            shim.mkdir()
            engine = directory / "fake.py"
            # The built executable accepts the real Pagura CLI.
            engine.write_text(FAKE_ENGINE.replace('mode = sys.argv[1]', 'mode = "ppm"').replace('args = sys.argv[2:]', 'args = sys.argv[1:]'))
            cargo = shim / "cargo"
            cargo.write_text("#!" + sys.executable + "\n" +
                "import os, pathlib, sys\n" +
                "assert '--offline' in sys.argv and '--locked' in sys.argv\n" +
                "target = pathlib.Path(sys.argv[sys.argv.index('--target-dir') + 1]) / 'release'\n" +
                "target.mkdir(parents=True)\n" +
                "binary = target / 'pagura'\n" +
                "binary.write_text(" + repr("#!" + sys.executable + "\n" + engine.read_text()) + ")\n" +
                "binary.chmod(0o755)\n")
            cargo.chmod(0o755)
            with patch.dict(os.environ, {"PATH": str(shim) + os.pathsep + os.environ.get("PATH", "")}), patch.object(measurement, "rss_wrapper", return_value=[]):
                with engine_tools.revision_engine("HEAD", repo) as (base, commit):
                    binary = Path(base[0])
                    self.assertFalse(binary.is_relative_to(repo))
                    self.assertEqual(len(commit), 40)
                    source = directory / "input.pdf"
                    source.write_bytes(b"fake input")
                    case = {"id": "revision-fixture", "path": str(source), "sha256": sha(source.read_bytes()), "dpi": 72, "page": 0}
                    bases = {"pagura": base, "revision": base}
                    policy = read_json(POLICY_PATH)
                    policy["bootstrap_resamples"] = 20
                    report = conversion.conversion_report([case], bases, {name: engine_tools.identify(name, command) for name, command in bases.items()},
                        policy, samples=4, warmups=1, seed=42017, input_digest=sha(b"manifest"), operation="regression", revision=commit, process_per_page=True)
                    cell = report["cells"][0]
                    self.assertEqual(cell["statistics"]["engine_ns"]["median_ratio"], 1)
                    self.assertEqual({tuple(s["order"]) for s in cell["samples"]},
                                     {("pagura", "revision"), ("revision", "pagura")})
                    self.assertFalse(gate(report)["pass"])
                self.assertFalse(binary.exists())
                worktrees = subprocess.check_output(["git", "worktree", "list", "--porcelain"], cwd=repo, text=True)
                self.assertEqual(worktrees.count("worktree "), 1)
                original_run = subprocess.run
                with patch.object(engine_tools.subprocess, "run") as run:
                    def dispatch(command, **kwargs):
                        if command[0] == "cargo":
                            raise subprocess.CalledProcessError(1, command)
                        return original_run(command, **kwargs)
                    run.side_effect = dispatch
                    with self.assertRaises(subprocess.CalledProcessError):
                        with engine_tools.revision_engine("HEAD", repo):
                            self.fail("failed build must not yield")
                worktrees = subprocess.check_output(["git", "worktree", "list", "--porcelain"], cwd=repo, text=True)
                self.assertEqual(worktrees.count("worktree "), 1)


# Document-only fake executable with independent bytes and known clock records.
DOCUMENT_ENGINE = r'''import json
import os
import sys
from pathlib import Path
mode = sys.argv[1]
args = sys.argv[2:]
if args[0] == 'convert':
    source, target, dpi = args[2:]
    first, last = 0, 1
elif mode == 'poppler':
    source, target = args[-2:]
    target = str(Path(target).parent)
    first = int(args[args.index('-f') + 1]) - 1
    last = int(args[args.index('-l') + 1]) - 1
else:
    source, target, dpi, first, last = args
    first, last = int(first), int(last)
if os.environ.get('W6_LAUNCH_LOG'):
    with open(os.environ['W6_LAUNCH_LOG'], 'a') as log:
        log.write(json.dumps({'mode': mode, 'first': first, 'last': last}) + '\n')
pages = []
for index in range(first, last + 1):
    pixels = b'abd' if mode == 'wrong-later' and index == last else b'abc'
    output = Path(target) / (f'page-{index + 1}.ppm' if mode == 'poppler' else f'page-{index + 1:04}.ppm')
    if not (mode == 'missing-later' and index == last):
        output.write_bytes(b'P6\n1 1\n255\n' + pixels)
    pages.append({'page': index, 'render_ns': 200, 'emit_ns': 300})
if mode == 'extra-page':
    (Path(target) / 'page-9999.ppm').write_bytes(b'P6\n1 1\n255\nabc')
if mode == 'duplicate-file':
    (Path(target) / 'page-1.ppm').write_bytes(b'P6\n1 1\n255\nabc')
if mode == 'duplicate-timer':
    pages[-1]['page'] = first
if mode == 'missing-timer':
    pages.pop()
if mode == 'reordered-timer':
    pages.reverse()
if mode == 'negative-later':
    pages[-1]['render_ns'] = -1
if mode != 'poppler':
    print(json.dumps({'schema': 1, 'open_ns': 100, 'pages': pages,
                      'total_ns': 1 if mode == 'bad-total' else 100 + len(pages) * 500}))
'''


class DocumentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.fake = self.directory / 'document.py'
        self.fake.write_text(DOCUMENT_ENGINE)
        self.source = self.directory / 'input.pdf'
        self.source.write_bytes(b'independent two-page input metadata')
        self.oracle = self.directory / 'oracle.ppm'
        self.oracle.write_bytes(b'P6\n1 1\n255\nabc')
        self.cases = [{'id': 'two-pages', 'path': str(self.source), 'sha256': sha(self.source.read_bytes()),
                       'page': page, 'page_count': 2, 'dpi': 72, 'oracle': str(self.oracle),
                       'oracle_sha256': sha(self.oracle.read_bytes())} for page in range(2)]
        self.policy = {**read_json(POLICY_PATH), 'bootstrap_resamples': 30}
        self.rss = patch.object(measurement, 'rss_wrapper', return_value=[])
        self.rss.start()
        self.addCleanup(self.rss.stop)

    def run_report(self, mode='correct', cases=None, baseline='pdfium', warmups=0):
        bases = {'pagura': [sys.executable, str(self.fake), 'correct'],
                 baseline: [sys.executable, str(self.fake), mode]}
        identities = {name: {'status': 'available'} for name in bases}
        return conversion.conversion_report(self.cases if cases is None else cases, bases, identities,
            self.policy, samples=2, warmups=warmups, seed=42, input_digest=sha(b'manifest'))

    def test_default_opens_once_per_document_and_counts_open_once(self):
        log = self.directory / 'launches.jsonl'
        with patch.dict(os.environ, {'W6_LAUNCH_LOG': str(log)}):
            report = self.run_report(warmups=1)
        launches = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual(len(launches), 6)  # 2 engines * (1 warmup + 2 samples), not * pages
        self.assertTrue(all((item['first'], item['last']) == (0, 1) for item in launches))
        self.assertEqual(report['settings']['page_process'], 'one open per document')
        self.assertEqual(len(report['document_cells']), 1)
        self.assertEqual(len(report['cells']), 2)
        document = report['document_cells'][0]
        for sample in document['samples']:
            for key in ('pagura', 'baseline'):
                result = sample[key]
                self.assertEqual(result['status'], 'ok')
                self.assertEqual(result['stages']['open_ns'], 100)
                self.assertEqual(result['stages']['render_ns'], 400)
                self.assertEqual(result['stages']['emit_ns'], 600)
                self.assertEqual(result['stages']['engine_ns'], 500)  # 100 + 2*200, not 2*(100+200)
                self.assertEqual(result['stages']['total_ns'], 1100)
        for cell in report['cells']:
            for sample in cell['samples']:
                self.assertNotIn('elapsed_ns', sample['pagura'])
                self.assertNotIn('peak_rss_bytes', sample['pagura'])
                self.assertEqual(sample['pagura']['stages'], {'render_ns': 200, 'emit_ns': 300})
                self.assertEqual(sample['agreement']['max_channel_error'], 0)
        self.assertFalse(gate(report)['pass'])
        self.assertIn('One process/open per document', reports.conversion_markdown(report))

    def test_every_page_verified_and_later_failures_invalidate_document_ratios(self):
        for mode, status in [('wrong-later', 'incorrect'), ('missing-later', 'invalid-output'),
                             ('extra-page', 'invalid-output'), ('duplicate-file', 'invalid-output')]:
            report = self.run_report(mode, warmups=1)
            document = report['document_cells'][0]
            self.assertEqual(len(document['samples']), 2)
            self.assertEqual(len(document['warmup_failures']), 1)
            self.assertEqual(document['samples'][0]['baseline']['status'], status, mode)
            self.assertEqual(document['statistics']['engine_ns']['status'], 'failed-output')
            self.assertEqual(report['cells'][0]['samples'][0]['baseline']['pixels_sha256'], sha(b'abc'))
            self.assertFalse(report['gate']['pass'])

    def test_later_timing_errors_never_become_successful_page_samples(self):
        for mode in ('duplicate-timer', 'missing-timer', 'reordered-timer', 'negative-later', 'bad-total'):
            report = self.run_report(mode)
            result = report['document_cells'][0]['samples'][0]['baseline']
            self.assertEqual(result['status'], 'invalid-timings', mode)
            self.assertTrue(all(page['status'] == 'invalid-timings' for page in result['pages']))
            self.assertEqual(report['document_cells'][0]['statistics']['engine_ns']['status'], 'failed-output')

    def test_poppler_document_process_only_uses_numeric_output_suffixes(self):
        report = self.run_report('poppler', baseline='poppler')
        result = report['document_cells'][0]['samples'][0]['baseline']
        self.assertEqual(result['status'], 'ok')
        self.assertEqual(result['stages_status'], 'unavailable')
        self.assertIsNone(result['stages'])
        self.assertTrue(all(page['status'] == 'ok' and 'stages' not in page for page in result['pages']))
        self.assertEqual(report['document_cells'][0]['statistics']['elapsed_ns']['status'], 'measured')
        self.assertEqual(report['document_cells'][0]['statistics']['engine_ns']['status'], 'unavailable-timing')

    def test_unverified_document_never_enters_release_even_with_agreement(self):
        cases = [{key: value for key, value in case.items() if key not in ('oracle', 'oracle_sha256')} for case in self.cases]
        report = self.run_report(cases=cases)
        self.assertEqual(report['document_cells'][0]['samples'][0]['pagura']['status'], 'unverified')
        self.assertTrue(all(cell['verification'] == 'unverified' for cell in report['cells']))
        self.assertTrue(all(item['max_channel_error'] == 0 for item in report['agreement']))
        self.assertFalse(gate(report)['pass'])

    def test_document_ranges_and_commands_are_equal_work(self):
        with self.assertRaisesRegex(ValueError, 'no range option'):
            self.run_report(cases=self.cases[1:])
        duplicate = [self.cases[0], self.cases[0]]
        with self.assertRaises(ValueError):
            documents.document_jobs(duplicate, {'pagura': []})
        cases = [{**case, 'page_count': 3} for case in self.cases]
        jobs = documents.document_jobs(cases, {'pdfium': [], 'mupdf': []})
        command = documents.document_command('pdfium', ['pdfium'], jobs[0], self.directory)
        self.assertEqual(command[-2:], ['0', '1'])
        command = documents.document_command('poppler', ['pdftoppm'], jobs[0], self.directory)
        self.assertNotIn('-singlefile', command)
        self.assertEqual(command[1:7], ['-r', '72', '-f', '1', '-l', '2'])
        command = documents.document_command('pagura', ['pagura'], self.cases, self.directory)
        self.assertEqual(command[1:3], ['convert', '--timings'])
        with self.assertRaisesRegex(ValueError, 'must be empty'):
            documents.measure_document(command, self.directory, {0: None}, self.policy,
                                       stages_required=True, image_sink={})

    def test_stage_parser_document_totals_and_page_set(self):
        record = {'schema': 1, 'open_ns': 10,
                  'pages': [{'page': 0, 'render_ns': 20, 'emit_ns': 30}, {'page': 1, 'render_ns': 40, 'emit_ns': 50}],
                  'total_ns': 150}
        self.assertEqual(parse_document_stages(json.dumps(record), [0, 1], 200)['engine_ns'], 70)
        for selected in ([1, 0], [0], [0, 0], []):
            with self.assertRaises(ValueError):
                parse_document_stages(json.dumps(record), selected, 200)
        with self.assertRaises(ValueError):
            parse_stages(json.dumps(record), 0, 200)

    def test_multiple_dpis_launch_document_jobs_and_cli_default(self):
        import bench as cli
        with patch('bench.run_conversion') as run:
            cli.main(['convert', '--engines', 'pagura,pdfium'])
            self.assertFalse(run.call_args.args[0].process_per_page)
            cli.main(['convert', '--process-per-page'])
            self.assertTrue(run.call_args.args[0].process_per_page)
        cases = self.cases + [{**case, 'dpi': 144} for case in self.cases]
        report = self.run_report(cases=cases)
        self.assertEqual(len(report['document_cells']), 2)
        self.assertEqual(len(report['cells']), 4)



class PythonAdapterBoundaryTests(unittest.TestCase):
    def test_public_api_fakes_pin_open_render_emit_boundaries_for_both_backends(self):
        import io
        import runpy
        import types
        for backend in ('pdfium', 'mupdf'):
            with self.subTest(backend=backend), tempfile.TemporaryDirectory() as directory:
                clock, events = [0], []
                class Image:
                    def convert(self, mode):
                        self.assert_mode = mode
                        clock[0] += 50
                        return self
                    def save(self, path, format):
                        clock[0] += 60
                        Path(path).write_bytes(b'P6\n1 1\n255\nabc')
                class Raster:
                    def to_pil(self):
                        return Image()
                    def tobytes(self, format):
                        clock[0] += 110
                        return b'P6\n1 1\n255\nabc'
                    def close(self):
                        pass
                class Page:
                    def __init__(self, index):
                        self.index = index
                    def render(self, **kwargs):
                        events.append(('render', self.index))
                        clock[0] += 40
                        return Raster()
                    get_pixmap = render
                    def close(self):
                        pass
                class Document:
                    def __init__(self, source):
                        clock[0] += 100
                        events.append(('open', source))
                    def __len__(self):
                        clock[0] += 20
                        events.append(('pages', 2))
                        return 2
                    @property
                    def page_count(self):
                        return len(self)
                    def __getitem__(self, index):
                        events.append(('load', index))
                        clock[0] += 30
                        return Page(index)
                    def close(self):
                        pass
                fake = types.SimpleNamespace(PdfDocument=Document, open=Document,
                                             Matrix=lambda *args: None, csRGB='RGB')
                stdout = io.StringIO()
                module = 'pypdfium2' if backend == 'pdfium' else 'pymupdf'
                with patch.dict(sys.modules, {module: fake}), patch.object(sys, 'argv',
                    ['python_engine.py', backend, 'fake-input', directory, '72', '0', '1']), \
                     patch('time.perf_counter_ns', side_effect=lambda: clock[0]), patch('sys.stdout', stdout):
                    runpy.run_path(str(engine_tools.ADAPTERS / 'python_engine.py'), run_name='__main__')
                record = json.loads(stdout.getvalue())
                self.assertEqual(record['open_ns'], 120)  # file+page list, no per-page loading
                self.assertEqual(record['pages'], [{'page': i, 'render_ns': 70, 'emit_ns': 110} for i in range(2)])
                self.assertEqual(record['total_ns'], 480)
                self.assertEqual(events, [('open', 'fake-input'), ('pages', 2), ('load', 0), ('render', 0), ('load', 1), ('render', 1)])
                self.assertEqual(sorted(path.name for path in Path(directory).iterdir()), ['page-0001.ppm', 'page-0002.ppm'])


if __name__ == "__main__":
    unittest.main()
