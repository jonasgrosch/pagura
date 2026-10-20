"""Reject report/retained-byte mismatches; synthetic records attest no runner."""
import copy
import os
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

from benchkit.artifacts import ArtifactStore
from benchkit.artifact_gate import (validate_artifact_run, verify_cell_artifacts,
                                    finish_artifact_verification)
from benchkit.config import POLICY_PATH, read_json, sha
from benchkit.engines import command_for
from benchkit.gate import gate
from benchkit.reports import markdown


class ArtifactGateTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        self.source, self.oracle = self.root / "input.pdf", self.root / "oracle.ppm"
        self.source.write_bytes(b"synthetic input not interpreted by a renderer")
        self.oracle.write_bytes(b"P6\n1 1\n255\nabc")
        self.case = {"id": "synthetic", "path": str(self.source), "sha256": sha(self.source.read_bytes()),
                     "oracle": str(self.oracle), "oracle_sha256": sha(self.oracle.read_bytes()),
                     "page": 0, "dpi": 72, "width": 1, "height": 1}
        self.policy = read_json(POLICY_PATH)
        run_id = str(uuid.uuid4())
        receipt = str(self.root / "runner.json")
        identity = {"status": "available", "version": "test-only", "command": [sys.executable],
                    "artifacts": {sys.executable: sha(Path(sys.executable).read_bytes())}}
        self.report = {"operation": "render-e2e", "samples_requested": 2, "warmups": 1,
                       "engine_identities": {"pagura": identity, "example": copy.deepcopy(identity)},
                       "runner": {"controlled": False, "coordination": {"run_id": run_id, "receipt": receipt}}}
        with patch.dict(os.environ, {"PAGURA_RUN_ID": run_id, "PAGURA_RUN_RECEIPT": receipt}):
            self.store = ArtifactStore.for_report(self.report, self.policy, directory=self.root / "artifacts")
        snapshot = self.store.snapshot(self.case)
        self.cell = {"case": "synthetic", "engine": "example", "operation": "render-e2e",
                     "pdf_sha256": self.case["sha256"], "oracle_pixels_sha256": sha(b"abc"),
                     "status": "measured", "samples": [], "warmup_samples": [], "warmup_failures": []}
        self.report["cells"] = [self.cell]
        for pair in (-1, 0, 1):
            order = ["example", "pagura"] if pair == 1 else ["pagura", "example"]
            row = {"pair": pair, "order": order}
            for side, engine in (("pagura", "pagura"), ("baseline", "example")):
                output = self.root / f"{engine}-{pair}.ppm"
                output.write_bytes(self.oracle.read_bytes())
                command = command_for(engine, identity["command"],
                    {**self.case, "path": str(snapshot)}, output)
                context = self.store.freeze_execution(cases=[self.case], snapshot=snapshot, engine=engine,
                    comparison_engine="example", pair=pair, order=order, command=command, output_paths={0: output})
                result = {"status": "ok", "exit_code": 0, "width": 1, "height": 1,
                          "output_sha256": sha(output.read_bytes()), "pixels_sha256": sha(b"abc"),
                          "elapsed_ns": 70 if engine == "pagura" else 100, "peak_rss_bytes": 50}
                self.store.capture(context, result, {0: output}, page=0)
                row[side] = result
            self.cell["warmup_samples" if pair < 0 else "samples"].append(row)
        self.store.finish()

    def verify(self):
        state = validate_artifact_run(self.report, self.policy)
        verify_cell_artifacts(self.report, self.cell, self.case, self.policy, state)
        finish_artifact_verification(self.report, state)

    def test_complete_retained_samples_and_warmups_verify_without_admission(self):
        self.verify()
        verdict = gate(self.report)
        self.assertFalse(verdict["pass"])
        self.assertTrue(any("independently pinned runner" in reason for reason in verdict["reasons"]))

    def test_gate_rechecks_artifacts_after_runner_authentication_and_keeps_matrix_block(self):
        # Isolate gate orchestration; runner receipt authentication has its own
        # raw-byte and pin tests. This stub grants no hardware/execution evidence.
        with patch("benchkit.gate.verify_runner_receipt", return_value={}), \
                patch("benchkit.gate.render_cases", return_value=[self.case]):
            verdict = gate(self.report)
            self.assertFalse(verdict["pass"])
            self.assertIn("cross-platform, resolution and concurrency aggregation is not implemented",
                          verdict["reasons"])
            self.assertFalse(any("artifact" in reason for reason in verdict["reasons"]))
            self.assertFalse(any("artifact" in reason for reason in verdict["comparisons"][0]["reasons"]))
            self.cell["warmup_samples"] = []
            bad = gate(self.report)
            self.assertTrue(any("retained artifact" in reason for reason in bad["comparisons"][0]["reasons"]))

    def test_missing_warmup_is_not_covered_by_report_warmup_count(self):
        self.cell["warmup_samples"] = []
        with self.assertRaisesRegex(ValueError, "sample/warmup"):
            self.verify()

    def test_failed_preflight_with_no_timing_still_has_reviewable_summary(self):
        self.cell["samples"][0]["pagura"] = {"status": "invalid-evidence", "exit_code": None}
        self.report.update(created_at="test-only", gate={"reasons": ["incomplete evidence"]})
        self.assertIn("unavailable", markdown(self.report))

    def test_tampered_raw_bytes_are_detected(self):
        blob = next((self.store.root / "blobs").iterdir())
        blob.chmod(0o600)
        blob.write_bytes(b"P6\n1 1\n255\nabd")
        with self.assertRaisesRegex(ValueError, "raw output"):
            self.verify()

    def test_cell_cannot_borrow_another_execution_context(self):
        self.cell["samples"][1]["pagura"] = self.cell["samples"][0]["pagura"]
        with self.assertRaisesRegex(ValueError, "reused"):
            self.verify()

    def test_frozen_context_must_match_independently_selected_request(self):
        binding = self.cell["samples"][0]["pagura"]["artifact_binding"]
        context = self.report["execution_contexts"][binding["context_sha256"]]
        context["dpi"] = 144
        with self.assertRaisesRegex(ValueError, "corpus/cell/sample"):
            self.verify()

    def test_missing_or_extra_context_is_incomplete(self):
        self.report["execution_contexts"]["unused"] = {}
        with self.assertRaisesRegex(ValueError, "unbound"):
            self.verify()

    def test_original_pdf_change_is_not_hidden_by_intact_snapshot(self):
        self.source.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "source PDF"):
            self.verify()

    def test_changed_reference_oracle_and_byte_accounting_fail(self):
        original = self.oracle.read_bytes()
        self.oracle.write_bytes(b"P6\n1 1\n255\nabd")
        with self.assertRaisesRegex(ValueError, "oracle"):
            self.verify()
        self.oracle.write_bytes(original)
        self.report["artifact_run"]["stored_bytes"] += 1
        with self.assertRaisesRegex(ValueError, "accounting"):
            self.verify()

    def test_incomplete_wrong_run_and_changed_normalizer_fail(self):
        original = copy.deepcopy(self.report["artifact_run"])
        for key, value in (("status", "recording"), ("run_id", str(uuid.uuid4())),
                           ("normalizer_sha256", "a" * 64), ("maximum_artifact_bytes", 12)):
            self.report["artifact_run"] = {**original, key: value}
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.verify()


if __name__ == "__main__":
    unittest.main()
