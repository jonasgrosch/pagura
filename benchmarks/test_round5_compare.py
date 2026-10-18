"""Reference comparisons use independent timings, failures and provenance."""
import contextlib
import copy
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import round5_compare as compare
from round5 import NATIVE, WASM, measurement, parse_args, result_exit_status

MACHINE = {"cpu_model": "test CPU", "platform": "test OS", "architecture": "x86_64"}
PROTOCOL = {"samples_requested": 2, "warmups": 1, "policy_sha256": "policy",
            "measurement_options": {"cpu_enabled": True, "rss_enabled": True, "rss_floor_bytes": 100},
            "round5": {"harness_sha256": {"benchmarks/process_measurement.py": "instrumentation"}}}


def timed(render, elapsed=100, status="unverified"):
    return {"status": status, "pages": [{"page": 0, "status": status, "stages": {"render_ns": render, "emit_ns": 7}}],
            "stages": {"engine_ns": render + 10, "open_ns": 10, "render_ns": render, "emit_ns": 7},
            "elapsed_ns": elapsed, "cpu_ns": elapsed - 1, "peak_rss_bytes": 1000}


def cell(case, page, dpi, engine, value):
    return {"case": case, "page": page, "dpi": dpi, "engine": engine, "status": "measured", "metrics": {"render_ns": value}}


class Round5CompareTests(unittest.TestCase):
    def test_parse_options_and_unchanged_defaults(self):
        defaults = parse_args(["--output", "out"])
        self.assertIsNone(defaults.engines)
        self.assertIsNone(defaults.reference)
        self.assertEqual(defaults.control, 0)
        self.assertEqual(defaults.seed, 52018)
        self.assertEqual((*NATIVE, *WASM)[0], "pagura")
        args = parse_args(["--output", "out", "--engines", "hayro,pagura", "--reference", "old", "--control", "4"])
        self.assertEqual(args.engines, ("pagura", "hayro"))
        self.assertEqual(args.reference, Path("old"))
        self.assertEqual(args.control, 4)
        for options in (["--engines", ""], ["--engines", "pagura,pagura"], ["--engines", "other"],
                        ["--control", "-1"], ["--control", "4"], ["--reference", "old"],
                        ["--reference", "old", "--control", "0"], ["--timebox-seconds", "0"]):
            with self.subTest(options=options), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                parse_args(["--output", "out", *options])

    def reference(self, root):
        identities = {"hayro": {"version": "1", "artifacts": {"binary": "abc"}}}
        report = {**copy.deepcopy(PROTOCOL), "suite": "round5-real-documents", "created_at": "date", "input_sha256": "manifest", "commit": "commit",
                  "engine_identities": identities, "samples_requested": 2, "completed_jobs": 1, "planned_jobs": 1,
                  "round5": {**copy.deepcopy(PROTOCOL["round5"]),
                             "selection": [{"case": "a", "page": 0, "dpi": 72, "content": "text-heavy"}]}}
        for name, value in (("conversion.json", report), ("engines.json", identities),
                            ("machine.json", {"observations": [MACHINE]}), ("status.json", {"status": "complete", "failed_cells": False})):
            (root / name).write_text(json.dumps(value))
        blocks = [{"pair": index, "case": "a", "dpi": 72, "runs": {"pagura": timed(value), "hayro": timed(value / 2)}}
                  for index, value in ((-1, 400), (0, 10), (1, 30))]
        (root / "blocks.jsonl").write_text("\n".join(map(json.dumps, blocks)) + "\n")
        return report

    def test_reference_loads_medians_and_pins_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.reference(root)
            reference = compare.load_reference(root, "manifest", MACHINE, PROTOCOL)
            self.assertEqual(reference["medians"]["a", 0, 72, "pagura"]["metrics"]["render_ns"], 20)
            self.assertEqual(reference["medians"]["a", None, 72, "hayro"]["metrics"]["engine_ns"], 20)
            pins = reference["provenance"]
            self.assertEqual(pins["run_id"], root.name)
            self.assertEqual(pins["manifest_sha256"], "manifest")
            for name in ("machine.json", "engines.json", "conversion.json"):
                self.assertEqual(pins["sha256"][name], compare.digest((root / name).read_bytes()))
            self.assertEqual(pins["blocks_sha256"], compare.digest((root / "blocks.jsonl").read_bytes()))

    def test_reference_refuses_manifest_machine_incomplete_and_identity_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.reference(root)
            with self.assertRaisesRegex(ValueError, "manifest"):
                compare.load_reference(root, "different", MACHINE, PROTOCOL)
            versioned = {**MACHINE, "os_version": {"status": "available", "stdout": "release one"}}
            (root / "machine.json").write_text(json.dumps({"observations": [versioned]}))
            with self.assertRaisesRegex(ValueError, "CPU model or OS"):
                compare.load_reference(root, "manifest", {**MACHINE, "os_version": {"status": "available", "stdout": "release two"}}, PROTOCOL)
            self.reference(root)
            for field in MACHINE:
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "CPU model or OS"):
                    compare.load_reference(root, "manifest", {**MACHINE, field: "different"}, PROTOCOL)
            with self.assertRaisesRegex(ValueError, "requires CPU"):
                compare.load_reference(root, "manifest", {}, PROTOCOL)
            for name, value, error in (("status.json", {"status": "incomplete"}, "complete"),
                                       ("engines.json", {}, "identities"),
                                       ("machine.json", {"observations": []}, "CPU model or OS")):
                self.reference(root)
                (root / name).write_text(json.dumps(value))
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, error):
                    compare.load_reference(root, "manifest", MACHINE, PROTOCOL)

    def test_reference_requires_identical_measurement_protocol(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            changes = (("samples_requested", 3), ("warmups", 2), ("policy_sha256", "different"),
                       ("measurement_options", {"cpu_enabled": False, "rss_enabled": True}))
            for field, value in changes:
                report = self.reference(root)
                report[field] = value
                (root / "conversion.json").write_text(json.dumps(report))
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, field):
                    compare.load_reference(root, "manifest", MACHINE, PROTOCOL)
            report = self.reference(root)
            report["round5"]["harness_sha256"]["benchmarks/process_measurement.py"] = "different"
            (root / "conversion.json").write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, "process_measurement_sha256"):
                compare.load_reference(root, "manifest", MACHINE, PROTOCOL)
            del report["round5"]["harness_sha256"]["benchmarks/process_measurement.py"]
            (root / "conversion.json").write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, "missing reference process_measurement.py"):
                compare.load_reference(root, "manifest", MACHINE, PROTOCOL)

    def test_reference_ignores_only_rss_floor_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = self.reference(root)
            report["measurement_options"]["rss_floor_bytes"] = 9000
            report["round5"]["harness_sha256"]["benchmarks/round5.py"] = "different runner, same accounting"
            (root / "conversion.json").write_text(json.dumps(report))
            compare.load_reference(root, "manifest", MACHINE, PROTOCOL)
            report["measurement_options"]["rss_launcher"] = ["other", "launcher"]
            (root / "conversion.json").write_text(json.dumps(report))
            with self.assertRaisesRegex(ValueError, "measurement_options"):
                compare.load_reference(root, "manifest", MACHINE, PROTOCOL)

    def test_smoke_cannot_use_full_reference_and_retiming_cannot_be_reference(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = self.reference(root)
            report.update(samples_requested=10, warmups=3)
            (root / "conversion.json").write_text(json.dumps(report))
            smoke = {**PROTOCOL, "samples_requested": 3, "warmups": 1}
            with self.assertRaisesRegex(ValueError, "samples_requested"):
                compare.load_reference(root, "manifest", MACHINE, smoke)
            for retiming in ({}, None):
                report = self.reference(root)
                report["retiming"] = retiming
                (root / "conversion.json").write_text(json.dumps(report))
                with self.assertRaisesRegex(ValueError, "re-timing run cannot"):
                    compare.load_reference(root, "manifest", MACHINE, PROTOCOL)

    def test_only_stable_comparisons_have_result_exit_status(self):
        for status in ("drifted", "incomplete", "not-requested"):
            self.assertEqual(result_exit_status(True, False, status), 3)
            self.assertEqual(result_exit_status(False, True, status), 3)
        self.assertEqual(result_exit_status(True, False, "stable"), 0)
        self.assertEqual(result_exit_status(True, True, "stable"), 2)
        self.assertEqual(result_exit_status(False, False, "stable"), 2)
        self.assertEqual(result_exit_status(True, False), 0)
        self.assertEqual(result_exit_status(True, True), 2)

    def test_unstable_summaries_withhold_comparison_tables(self):
        reference = self.controls()
        controls = compare.select_controls(reference, 4)
        result = compare.comparison(reference["report"]["round5"]["selection"], {}, reference, ("pagura",))
        for status in ("drifted", "incomplete", "not-requested", "stable"):
            drift = compare.control_drift(controls, reference["medians"], reference["medians"])
            drift["status"] = status
            text = compare.markdown(result, drift, {"run_id": "old"})
            if status == "stable":
                self.assertIn("Category | DPI | Cells", text)
                self.assertNotIn("NOT A RESULT", text)
            else:
                self.assertIn("NOT A RESULT", text)
                self.assertNotIn("Category | DPI | Cells", text)
                self.assertNotIn("Pagura ÷ best", text)

    def test_drift_checks_engine_and_elapsed_even_when_render_is_stable(self):
        reference = self.controls()
        controls = compare.select_controls(reference, 4)
        for metric in compare.DRIFT_METRICS:
            current = copy.deepcopy(reference["medians"])
            for value in current.values():
                value["metrics"][metric] = 110
            drift = compare.control_drift(controls, current, reference["medians"])
            self.assertEqual(drift["status"], "drifted")
            self.assertEqual(drift["metrics"][metric]["status"], "drifted")
            for other in set(compare.DRIFT_METRICS) - {metric}:
                self.assertEqual(drift["metrics"][other]["status"], "stable")
        for metric in ("engine_ns", "elapsed_ns"):
            current = copy.deepcopy(reference["medians"])
            for value in current.values():
                value["metrics"].pop(metric)
            drift = compare.control_drift(controls, current, reference["medians"])
            self.assertEqual(drift["status"], "incomplete")

    def test_document_drift_counts_each_document_once(self):
        controls = [{"case": "a", "page": page, "dpi": 72, "content": "mixed"} for page in (0, 1)]
        values = {("a", page, 72, "hayro"): {"metrics": {metric: 100 for metric in compare.DRIFT_METRICS}}
                  for page in (0, 1, None)}
        drift = compare.control_drift(controls, values, values)
        self.assertEqual(drift["status"], "stable")
        self.assertEqual(len(drift["metrics"]["render_ns"]["cells"]), 2)
        for metric in ("engine_ns", "elapsed_ns"):
            self.assertEqual(len(drift["metrics"][metric]["cells"]), 1)

    def test_best_excludes_both_stored_pagura_wasm_engines(self):
        selection = [{"case": "a", "page": 0, "dpi": 72, "content": "mixed"}]
        values = [cell("a", 0, 72, name, value) for name, value in
                  (("pagura", 10), ("hayro", 5), ("v8-wasi", 1), ("wasmtime", 2))]
        result = compare.comparison(selection, {compare.key(value): value for value in values}, None,
                                    ("pagura", "hayro", "v8-wasi", "wasmtime"))
        self.assertEqual(result["rows"][0]["ratios"]["best"], 2)
        self.assertEqual(result["rows"][0]["ratios"]["v8-wasi"], 10)

    def test_control_identity_allows_moved_paths_but_not_changed_binary(self):
        old = {"version": "1", "artifacts": {"/old/bin": "abc", "/old/lock": "def"}}
        compare.check_control_identity({"version": "1", "artifacts": {"/new/bin": "abc", "/new/lock": "def"}}, old)
        with self.assertRaisesRegex(ValueError, "control engine identity"):
            compare.check_control_identity({"version": "1", "artifacts": {"/new/bin": "changed", "/new/lock": "def"}}, old)

    def test_failed_samples_and_warmups_are_not_filtered(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.reference(root)
            records = [json.loads(line) for line in (root / "blocks.jsonl").read_text().splitlines()]
            for pair in (-1, 0):
                broken = copy.deepcopy(records)
                next(block for block in broken if block["pair"] == pair)["runs"]["hayro"]["status"] = "timeout"
                (root / "blocks.jsonl").write_text("\n".join(map(json.dumps, broken)) + "\n")
                values = compare.medians(root, 2)
                self.assertEqual(values["a", 0, 72, "hayro"]["status"], "failed")
                self.assertIsNone(values["a", 0, 72, "hayro"]["metrics"]["render_ns"])
                self.assertEqual(values["a", 0, 72, "pagura"]["metrics"]["render_ns"], 20)

    def controls(self):
        selection = [{"case": f"{index}-{dpi}-{page}", "page": page, "dpi": dpi, "content": content}
                     for index, content in enumerate(compare.CONTENT) for dpi in (72, 150, 300) for page in range(2)]
        values = {}
        for item in selection:
            for page in (item["page"], None):
                value = cell(item["case"], page, item["dpi"], "hayro", 100)
                value["metrics"].update(engine_ns=100, elapsed_ns=100)
                values[compare.key(value)] = value
        return {"report": {"round5": {"selection": selection}}, "medians": values}

    def test_controls_mix_categories_and_dpi_deterministically(self):
        reference = self.controls()
        selected = compare.select_controls(reference, 4)
        self.assertEqual({item["content"] for item in selected}, set(compare.CONTENT))
        self.assertEqual({item["dpi"] for item in selected}, {72, 150, 300})
        reference["report"]["round5"]["selection"].reverse()
        self.assertEqual(selected, compare.select_controls(reference, 4))
        self.assertEqual(len(compare.select_controls(reference, 24)), 24)
        with self.assertRaisesRegex(ValueError, "count exceeds"):
            compare.select_controls(reference, 25)

    def test_drift_thresholds_and_missing_control(self):
        reference = self.controls()
        controls = compare.select_controls(reference, 4)
        for ratio, expected in ((1, "stable"), (1.03, "stable"), (.97, "stable"), (1.031, "drifted"), (.969, "drifted")):
            current = {key: {**value, "metrics": {metric: 100 * ratio for metric in compare.DRIFT_METRICS}}
                       for key, value in reference["medians"].items()}
            drift = compare.control_drift(controls, current, reference["medians"])
            self.assertTrue(all(abs(value["median_ratio"] - ratio) < 1e-12 for value in drift["metrics"].values()))
            self.assertEqual(drift["status"], expected)
        self.assertEqual(compare.control_drift(controls, {}, reference["medians"])["status"], "incomplete")
        self.assertEqual(compare.control_drift([], {}, {})["status"], "not-requested")

    def test_summary_uses_ratio_of_medians_then_median_of_cell_ratios(self):
        selection = [{"case": name, "page": 0, "dpi": 72, "content": "text-heavy"} for name in ("a", "b")]
        old = [cell("a", 0, 72, "pagura", 20), cell("b", 0, 72, "pagura", 40),
               cell("a", 0, 72, "hayro", 10), cell("b", 0, 72, "hayro", 100),
               cell("a", 0, 72, "pdfium", 20), cell("b", 0, 72, "pdfium", 20)]
        current = [cell("a", 0, 72, "pagura", 10), cell("b", 0, 72, "pagura", 20), cell("a", 0, 72, "hayro", 1000)]
        result = compare.comparison(selection, {compare.key(value): value for value in current},
                                    {"medians": {compare.key(value): value for value in old}}, ("pagura",))
        row = result["rows"][0]
        self.assertEqual(row["ratios"]["hayro"], .6)
        self.assertEqual(row["ratios"]["pdfium"], .75)
        self.assertEqual(row["ratios"]["best"], 1)
        self.assertEqual(row["ratios"]["reference_pagura"], .5)
        current.pop(1)
        missing = compare.comparison(selection, {compare.key(value): value for value in current},
                                     {"medians": {compare.key(value): value for value in old}}, ("pagura",))
        self.assertIsNone(missing["rows"][0]["ratios"]["hayro"])
        controls = self.controls()
        current_controls = {key: {**value, "metrics": {metric: 104 for metric in compare.DRIFT_METRICS}}
                            for key, value in controls["medians"].items()}
        drift = compare.control_drift(compare.select_controls(controls, 4), current_controls, controls["medians"])
        text = compare.markdown(result, drift, {"run_id": "old"})
        self.assertIn("DRIFTED", text)
        self.assertIn("exceeds ±3%", text)
        self.assertIn("NOT A RESULT", text)
        self.assertNotIn("Pagura ÷ reference Pagura", text)

    def test_process_summary_counts_document_once_and_compares_poppler(self):
        selection = [{"case": "a", "page": page, "dpi": 72, "content": "mixed"} for page in (0, 1)]
        values = [cell("a", None, 72, "pagura", 0), cell("a", None, 72, "poppler", 0)]
        values[0]["metrics"] = {"engine_ns": 50, "elapsed_ns": 100}
        values[1]["metrics"] = {"elapsed_ns": 200}
        result = compare.comparison(selection, {compare.key(value): value for value in values}, None, ("pagura", "poppler"))
        process = next(row for row in result["rows"] if row["metric"] == "elapsed_ns")
        self.assertEqual(process["cells"], 1)
        self.assertEqual(process["ratios"]["poppler"], .5)
        self.assertEqual(process["ratios"]["best"], .5)
        engine = next(row for row in result["rows"] if row["metric"] == "engine_ns")
        self.assertIsNone(engine["ratios"]["poppler"])
        self.assertIsNone(engine["ratios"]["best"])

    def test_reference_refuses_duplicate_and_failed_raw_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.reference(root)
            original = (root / "blocks.jsonl").read_text()
            (root / "blocks.jsonl").write_text(original + original.splitlines()[-1] + "\n")
            with self.assertRaisesRegex(ValueError, "duplicate"):
                compare.load_reference(root, "manifest", MACHINE, PROTOCOL)
            records = [json.loads(line) for line in original.splitlines()]
            records[-1]["runs"]["hayro"]["status"] = "timeout"
            (root / "blocks.jsonl").write_text("\n".join(map(json.dumps, records)) + "\n")
            with self.assertRaisesRegex(ValueError, "failed samples"):
                compare.load_reference(root, "manifest", MACHINE, PROTOCOL)

    def test_measurement_runs_only_selected_and_control_engines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pdf = root / "source.pdf"
            pdf.write_bytes(b"independent input")
            import hashlib
            pages = [[{"id": name, "page": 0, "dpi": 72, "path": str(pdf), "sha256": hashlib.sha256(pdf.read_bytes()).hexdigest()}]
                     for name in ("main", "control")]
            report = {"cells": [], "document_cells": [], "runtime_parity": []}
            args = SimpleNamespace(seed=1, engines=("pagura",), main_jobs={("main", 72)}, control_jobs={("control", 72)},
                                   samples=3, warmups=1, smoke=True, deadline=float("inf"))
            measured = []
            def measure(command, *args, **kwargs):
                measured.append(command[0])
                return timed(10)
            with patch("round5.idle_record"), patch("round5.expected_pages", return_value={0: None}), \
                 patch("round5.measure_document", side_effect=measure), contextlib.redirect_stdout(io.StringIO()):
                measurement(report, pages, {"pagura": ["pagura"], "hayro": ["hayro"]}, None, None, None,
                            {"timeout_seconds": 1}, root, {}, args)
            self.assertEqual(measured.count("pagura"), 4)
            self.assertEqual(measured.count("hayro"), 4)
            self.assertEqual(report["completed_jobs"], 2)
            self.assertEqual(report["cells"], [])
            self.assertEqual(len((root / "processes.jsonl").read_text().splitlines()), 8)
            default_output = root / "default"
            default_output.mkdir()
            args.engines = None
            args.main_jobs = {(name, 72) for name in ("main", "control")}
            args.control_jobs = set()
            report = {"cells": [], "document_cells": [], "runtime_parity": []}
            measured.clear()
            with patch("round5.idle_record"), patch("round5.expected_pages", return_value={0: None}), \
                 patch("round5.measure_document", side_effect=measure), \
                 patch("round5.wasm_command", side_effect=lambda name, *args: [name]), \
                 contextlib.redirect_stdout(io.StringIO()):
                measurement(report, pages, {name: [name] for name in NATIVE}, None, None, None,
                            {"timeout_seconds": 1}, default_output, {}, args)
            for engine in (*NATIVE, *WASM):
                self.assertEqual(measured.count(engine), 8)
            self.assertEqual(len(report["document_cells"]), 2 * (len(NATIVE) + len(WASM) - 1))
            self.assertTrue(all(len(value["samples"]) == 3 for value in report["document_cells"]))
            self.assertEqual(len(report["runtime_parity"]), 2 * len(WASM) * 4)


if __name__ == "__main__":
    unittest.main()
