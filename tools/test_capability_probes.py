"""Harness controls run offline, without importing or executing competitor code."""
import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import capability_probes as probes


class ProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = probes.read_json(probes.ROOT / probes.MANIFEST)
        cls.inventory = probes.read_json(probes.ROOT / probes.LEDGER)

    def setUp(self):
        self.case = copy.deepcopy(self.manifest["cases"][0])

    def test_subject_is_additive_and_its_failed_oracle_fails_the_job(self):
        inventory = probes.subject_inventory(self.inventory)
        self.assertNotIn("pagura", self.inventory["engines"])
        self.assertEqual(inventory["engines"], [*self.inventory["engines"], "pagura"])
        with self.assertRaisesRegex(ValueError, "lost a baseline"):
            probes.validate_report(self.report(), self.manifest, inventory)
        def observed(command, case, directory):
            if case["oracle"] == "reject-invalid":
                return {"outcome": "pass", "reason": "stub rejection"}
            return {"outcome": "pass", "observed": case["expected"]}
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "report.json"
            with patch.object(probes, "service_commands", return_value={"pagura": ["stub"]}), \
                 patch.object(probes, "service_identity", return_value={"status": "available"}), \
                 patch.object(probes, "invoke", side_effect=observed):
                self.assertTrue(probes.run(self.manifest, inventory, output, ("pagura",)))
                report = probes.read_json(output)
                self.assertEqual(len(report["results"]), 70)
                self.assertEqual(sum(row["outcome"] == "not-run" for row in report["results"]), 60)
            with patch.object(probes, "service_commands", return_value={"pagura": ["stub"]}), \
                 patch.object(probes, "service_identity", return_value={"status": "available"}), \
                 patch.object(probes, "invoke", return_value={"outcome": "incorrect", "observed": None}):
                self.assertFalse(probes.run(self.manifest, inventory, output, ("pagura",)))

    def test_resource_limits_never_pass_an_invalid_input_control(self):
        negative = next(row for row in self.manifest["cases"] if row["oracle"] == "reject-invalid")
        for status in ("limit", "timeout"):
            self.assertEqual(probes.adjudicate(negative, {"schema": 1, "status": status, "reason": "bounded"})["outcome"], status)

    def test_manifest_owns_operations_and_empty_and_invalid_controls(self):
        probes.validate_manifest(self.manifest, self.inventory)
        for removed in ("pages.invalid", "attachments.empty"):
            broken = copy.deepcopy(self.manifest)
            broken["cases"] = [row for row in broken["cases"] if row["id"] != removed]
            with self.subTest(removed=removed), self.assertRaisesRegex(ValueError, "missing"):
                probes.validate_manifest(broken, self.inventory)
        broken = copy.deepcopy(self.manifest)
        broken["cases"][0]["capability"] = "unknown.operation"
        with self.assertRaisesRegex(ValueError, "unassigned"):
            probes.validate_manifest(broken, self.inventory)

    def test_independent_oracle_rejects_wrong_values_not_just_successful_exit(self):
        response = {"schema": 1, "status": "ok", "value": self.case["expected"]}
        self.assertEqual(probes.adjudicate(self.case, response)["outcome"], "pass")
        response["value"] = {"count": 2, "sizes": [[200, 100]]}
        self.assertEqual(probes.adjudicate(self.case, response)["outcome"], "incorrect")
        response["value"] = {"count": True, "sizes": [[200, 100]]}
        self.assertEqual(probes.adjudicate(self.case, response)["outcome"], "incorrect")
        self.assertFalse(probes.same_value(float("nan"), float("nan")))
        self.assertFalse(probes.same_value(float("inf"), float("inf")))
        self.assertFalse(probes.same_value(10 ** 400, 1))

    def test_rejection_is_only_a_pass_for_invalid_input(self):
        response = {"schema": 1, "status": "rejected", "reason": "EngineOpenError"}
        self.assertEqual(probes.adjudicate(self.case, response)["outcome"], "rejected")
        negative = next(row for row in self.manifest["cases"] if row["oracle"] == "reject-invalid")
        self.assertEqual(probes.adjudicate(negative, response)["outcome"], "pass")
        response = {"schema": 1, "status": "ok", "value": None}
        self.assertEqual(probes.adjudicate(negative, response)["outcome"], "incorrect")

    def test_unavailable_and_broken_adapters_never_mean_unsupported(self):
        for status in ("adapter-error", "adapter-unavailable", "engine-unavailable"):
            result = probes.adjudicate(self.case, {"schema": 1, "status": status, "reason": "control"})
            self.assertEqual(result["outcome"], status)
        missing_tag = {"schema": 1, "status": "unsupported", "reason": "unknown"}
        self.assertEqual(probes.adjudicate(self.case, missing_tag)["outcome"], "adapter-error")
        explicit = {**missing_tag, "feature": "documented-engine-refusal"}
        self.assertEqual(probes.adjudicate(self.case, explicit)["outcome"], "unsupported")
        for response in (None, [], {"schema": True}, {"schema": 1, "status": "ok"}):
            self.assertEqual(probes.adjudicate(self.case, response)["outcome"], "adapter-error")

    def test_manifest_is_not_derived_from_engine_or_generator_output(self):
        attachments = next(row for row in self.manifest["cases"] if row["id"] == "attachments.services")
        self.assertEqual(attachments["expected"], [{"name": "probe.txt", "sha256": probes.sha(b"Synthetic attachment.\n")}])
        content = probes.fixture("services")
        xref = int(content.split(b"startxref\n")[1].splitlines()[0])
        self.assertTrue(content[xref:].startswith(b"xref\n"))
        entries = content[xref:].splitlines()[3:11]
        for index, entry in enumerate(entries, 1):
            offset = int(entry[:10])
            self.assertTrue(content[offset:].startswith(f"{index} 0 obj\n".encode()))
        self.assertIn(b"(Parity probe)", content)
        self.assertNotIn(b"/UF 8 0 R", content)
        self.assertIn(b"/UF 8 0 R", probes.fixture("complete-ef"))
        self.assertNotIn(b"/EmbeddedFiles", probes.fixture("empty"))
        self.assertFalse(probes.fixture("invalid").startswith(b"%PDF"))

    def report(self):
        return {"schema": 1, "cases": self.manifest["cases"],
                "selected_engines": list(self.inventory["engines"]),
                "revision": "source-commit", "scope": self.manifest["scope"], "source_sha256": {"source": "hash"},
                "fixtures": {"fixture": "hash"}, "workflow_run": "run", "environment": {"system": "stub"},
                "generated_at": "time", "command": ["stub"],
                "engines": {engine: {"status": "available"} for engine in self.inventory["engines"]},
                "results": [{"engine": engine, "case": case["id"], "outcome": "adapter-unavailable"}
                            for engine in self.inventory["engines"] for case in self.manifest["cases"]]}

    def test_missing_cells_duplicate_cells_and_unattested_passes_fail(self):
        report = self.report()
        probes.validate_report(report, self.manifest, self.inventory)
        report["results"].pop()
        with self.assertRaisesRegex(ValueError, "missing probe cells"):
            probes.validate_report(report, self.manifest, self.inventory)
        report = self.report()
        report["results"].append(report["results"][0])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            probes.validate_report(report, self.manifest, self.inventory)
        report = self.report()
        report["results"][0].update(outcome="pass", observed={"wrong": "value"})
        with self.assertRaisesRegex(ValueError, "matching oracle"):
            probes.validate_report(report, self.manifest, self.inventory)
        report["results"][0]["observed"] = self.case["expected"]
        report["engines"][report["results"][0]["engine"]]["status"] = "unavailable"
        with self.assertRaisesRegex(ValueError, "engine identity"):
            probes.validate_report(report, self.manifest, self.inventory)

    def test_rejection_pass_requires_rejection_evidence(self):
        report = self.report()
        negative = next(row for row in report["results"] if row["case"] == "pages.invalid")
        negative["outcome"] = "pass"
        with self.assertRaisesRegex(ValueError, "rejection evidence"):
            probes.validate_report(report, self.manifest, self.inventory)
        negative["reason"] = "EngineOpenError"
        probes.validate_report(report, self.manifest, self.inventory)

    def test_measurement_gate_preserves_competitor_differences_but_fails_broken_adapters(self):
        for engine in self.inventory["engines"]:
            for case in self.manifest["cases"]:
                gap = (engine, case["operation"]) in probes.UNIMPLEMENTED
                for outcome in probes.OUTCOMES:
                    expected = outcome == "adapter-unavailable" if gap else outcome in {"pass", "incorrect", "rejected", "unsupported"}
                    self.assertEqual(probes.observation_complete(engine, case, {"outcome": outcome}), expected)
        wrong = probes.adjudicate(self.case, {"schema": 1, "status": "ok", "value": []})
        self.assertTrue(probes.observation_complete("pdfjs", self.case, wrong))
        self.assertEqual(wrong["outcome"], "incorrect")

    def split_reports(self):
        reports = []
        for selected in ([engine for engine in self.inventory["engines"] if engine != "quartz"], ["quartz"]):
            report = self.report()
            report["selected_engines"] = selected
            for row in report["results"]:
                if row["engine"] not in selected:
                    row["outcome"] = "not-run"
            reports.append(report)
        return reports

    def test_combination_requires_same_evidence_and_complete_disjoint_engines(self):
        reports = self.split_reports()
        combined = probes.combine(reports, self.manifest, self.inventory)
        self.assertEqual(len(combined["results"]), len(self.manifest["cases"]) * len(self.inventory["engines"]))
        self.assertNotIn("not-run", {row["outcome"] for row in combined["results"]})
        for field in ("revision", "source_sha256", "fixtures", "workflow_run"):
            broken = copy.deepcopy(reports)
            broken[1][field] = "different"
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, field):
                probes.combine(broken, self.manifest, self.inventory)
        with self.assertRaisesRegex(ValueError, "overlapping"):
            probes.combine([reports[0], reports[0]], self.manifest, self.inventory)
        with self.assertRaisesRegex(ValueError, "missing baseline"):
            probes.combine([reports[0]], self.manifest, self.inventory)
        reports[0]["results"][0]["outcome"] = "not-run"
        with self.assertRaisesRegex(ValueError, "selected engine/result"):
            probes.validate_report(reports[0], self.manifest, self.inventory)

    def test_subprocess_failure_controls_do_not_run_an_engine(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with patch.object(probes.subprocess, "run", side_effect=subprocess.TimeoutExpired("stub", 20)):
                self.assertEqual(probes.invoke(["stub"], self.case, directory)["outcome"], "timeout")
            with patch.object(probes.subprocess, "run", side_effect=FileNotFoundError):
                self.assertEqual(probes.invoke(["stub"], self.case, directory)["outcome"], "engine-unavailable")
            def stub(command, *, stdout, stderr, timeout, cwd):
                stdout.write(b"not JSON")
                return subprocess.CompletedProcess(command, 0)
            with patch.object(probes.subprocess, "run", side_effect=stub):
                self.assertEqual(probes.invoke(["stub"], self.case, directory)["outcome"], "adapter-error")
            def success(command, *, stdout, stderr, timeout, cwd):
                stdout.write(json.dumps({"schema": 1, "status": "ok", "value": self.case["expected"]}).encode())
                return subprocess.CompletedProcess(command, 0)
            with patch.object(probes.subprocess, "run", side_effect=success):
                self.assertEqual(probes.invoke(["stub"], self.case, directory)["outcome"], "pass")

    def test_public_report_hides_machine_paths_and_raw_failure_logs(self):
        source = str(probes.ROOT / probes.ADAPTER)
        binary = str(probes.CACHE / "venv/bin/python")
        version = '{"wrapper": "pinned-wrapper", "engine": "pinned-engine"}\nwarning: /private/device-path'
        identity = probes.public_identity({"status": "available", "version": version,
                                           "command": [binary, source], "artifacts": {binary: "abc", source: "def"}})
        self.assertEqual(identity["version"], {"wrapper": "pinned-wrapper", "engine": "pinned-engine"})
        self.assertNotIn("/private/device-path", json.dumps(identity))
        self.assertNotIn(str(probes.ROOT), json.dumps(identity))
        self.assertNotIn(str(probes.CACHE), json.dumps(identity))
        self.assertNotIn("sensitive log", json.dumps(probes.public_identity({"status": "unavailable", "reason": "sensitive log"})))


if __name__ == "__main__":
    unittest.main()
