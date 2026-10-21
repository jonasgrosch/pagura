"""Focused synthetic checks for the standalone auditor; no renderer is invoked."""
import copy
from pathlib import Path
import tempfile
import unittest

import audit


class Images(unittest.TestCase):
    def test_payload_whitespace_and_rgb_alpha_unavailable(self):
        actual = audit.image(b"P6\n# comment\n1 1\n255\n\x0a\x20\x23")
        self.assertEqual(actual[:3], (1, 1, b"\x0a\x20\x23"))
        self.assertEqual(actual[3]["status"], "unavailable")

    def test_straight_and_premultiplied_use_independent_white_expectation(self):
        straight = audit.image(b"RGBA8 1 1 straight\n" + bytes([200, 100, 50, 128]))
        premul = audit.image(b"RGBA8 1 1 premultiplied\n" + bytes([100, 50, 25, 128]))
        self.assertEqual(straight[2], bytes([227, 177, 152]))
        self.assertEqual(premul[2], bytes([227, 177, 152]))
        self.assertEqual(straight[3]["nonopaque_pixels"], 1)
        self.assertEqual(premul[3]["minimum"], 128)

    def test_pam_alpha_is_observed_without_alpha_accuracy_claim(self):
        raw = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n" + bytes(4)
        actual = audit.image(raw)
        self.assertEqual(actual[2], b"\xff\xff\xff")
        self.assertEqual(actual[3]["maximum"], 0)
        self.assertIn("no independent", actual[3]["accuracy"])

    def test_malformed_premultiplication_and_trailing_data_fail(self):
        for raw in (b"RGBA8 1 1 premultiplied\n" + bytes([129, 0, 0, 128]),
                    b"P6\n1 1\n255\n\0\0\0\0"):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                audit.image(raw)

    def test_rgb_difference_is_not_parity_or_tolerance_relaxation(self):
        expected = audit.image(b"P6\n1 1\n255\n\0\0\0")
        actual = audit.image(b"P6\n1 1\n255\n\0\1\0")
        difference = audit.compare(actual, expected)
        self.assertEqual(difference["status"], "incorrect")
        self.assertEqual(difference["different_channels"], 1)
        self.assertEqual(difference["channel_maxima"], [0, 1, 0])


class Statistics(unittest.TestCase):
    def test_missing_pair_or_stage_never_yields_success_subset_estimate(self):
        pairs = [{"pagura": {"metrics": {"elapsed_ns": n}},
                  "baseline": {"metrics": {"elapsed_ns": n * 2}}} for n in [10, 20, 30, 40, 50]]
        summary = audit.summarize(pairs, True)
        self.assertEqual(summary["elapsed_ns"]["median_ratio"], 0.5)
        self.assertEqual(summary["elapsed_ns"]["sides"]["pagura"]["median_ns"], 30)
        self.assertEqual(summary["elapsed_ns"]["sides"]["pagura"]["p95_ns"], 48)
        self.assertEqual(summary["render_ns"]["status"], "unavailable")
        del pairs[2]["baseline"]["metrics"]["elapsed_ns"]
        self.assertEqual(audit.summarize(pairs, True)["elapsed_ns"]["status"], "unavailable")
        self.assertNotIn("median_ratio", audit.summarize(pairs, True)["elapsed_ns"])

    def test_unqualified_outputs_keep_all_timings_without_quality_ratio(self):
        pairs = [{"pagura": {"metrics": {"elapsed_ns": n}},
                  "baseline": {"metrics": {"elapsed_ns": 100}}} for n in [1, 2, 3, 4, 1000]]
        summary = audit.summarize(pairs, False)["elapsed_ns"]
        self.assertEqual(summary["status"], "all-pair-unqualified")
        self.assertEqual(summary["sides"]["pagura"]["count"], 5)
        self.assertEqual(summary["sides"]["pagura"]["maximum_ns"], 1000)
        self.assertNotIn("median_ratio", summary)

    def test_invalid_stage_arithmetic_and_boolean_timings_fail(self):
        for measurement in ({"elapsed_ns": True}, {"elapsed_ns": 1 << 80},
            {"elapsed_ns": 100, "stages": {"open_ns": 40, "render_ns": 60, "engine_ns": 100,
                                            "emit_ns": 1, "total_ns": 100}},
            {"elapsed_ns": 100, "stages": {"open_ns": 10, "render_ns": 20, "engine_ns": 31,
                                            "emit_ns": 1, "total_ns": 50}}):
            with self.subTest(measurement=measurement), self.assertRaises(ValueError):
                audit.stages(measurement, "pagura")
        self.assertEqual(audit.stages({"elapsed_ns": 100, "stages": None}, "poppler")[1], "unavailable")


class Receipts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name).resolve()
        self.source = b"independent synthetic input; no engine reads this"
        self.case = {"id": "synthetic", "sha256": audit.sha(self.source)}
        self.identity = {"status": "available", "command": ["/fixture/pdftoppm"], "artifacts": {}}
        self.report = {"artifact_run": {"run_id": "fixture", "normalizer_sha256": "normalizer"},
                       "engine_identities": {"poppler": self.identity}, "execution_contexts": {}}
        self.auditor = audit.Auditor(self.directory, self.directory, self.report,
                                     {"maximum_output_bytes": 4096}, {})
        self.auditor.artifact_root = self.directory
        self.auditor.artifact_maximum = 65536
        self.auditor.identity_valid = {"poppler": True}
        self.auditor.oracles = {"synthetic": audit.image(b"P6\n1 1\n255\n\0\0\0")}
        self.put(f"inputs/{audit.sha(self.source)}.pdf", self.source)

    def put(self, relative, raw):
        path = self.directory / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)

    def receipt(self, *, status="ok", missing=False, context_page=0):
        snapshot = f"inputs/{self.case['sha256']}.pdf"
        command = ["/fixture/pdftoppm", "-f", "1", "-l", "1", "-singlefile", "-r", "72",
                   str(self.directory / snapshot), "/fixture/output"]
        context = {"schema": 1, "run_id": "fixture", "operation": "conversion", "case": "synthetic",
                   "source_pdf_sha256": self.case["sha256"], "pages": [context_page], "dpi": 72,
                   "engine": "poppler", "engine_identity_sha256": audit.digest(self.identity),
                   "comparison_engine": "poppler", "pair": 0, "phase": "sample", "order": ["pagura", "poppler"],
                   "render_settings": {"background": "white", "output": "RGB8 over white", "scope": "adapter-request"},
                   "references": {}, "input_snapshot": snapshot, "output_paths": {"0": "/fixture/output.ppm"},
                   "command": command, "command_sha256": audit.digest(command)}
        measurement = {"status": status, "exit_code": 0 if status == "ok" else 9, "elapsed_ns": 100,
                       "peak_rss_bytes": 1024, "rss_status": "available"}
        if missing:
            output = {"error": "fixture process produced no output"}
            measurement["diagnostic"] = "retained timeout tail"
        else:
            raw = b"P6\n1 1\n255\n\0\0\0"
            output = {"raw_sha256": audit.sha(raw), "raw_bytes": len(raw), "blob": f"blobs/{audit.sha(raw)}.raw",
                      "width": 1, "height": 1, "pixels_sha256": audit.sha(bytes(3)),
                      "normalized_ppm_sha256": audit.sha(raw), "normalization": "rgb8-over-white-v1",
                      "normalizer_sha256": "normalizer"}
            self.put(output["blob"], raw)
            if status == "ok":
                measurement.update(width=1, height=1, pixels_sha256=output["pixels_sha256"],
                                   output_sha256=output["raw_sha256"], stages=None, stages_status="unavailable")
        context_key = audit.digest(context)
        self.report["execution_contexts"][context_key] = context
        raw_receipt = audit.canonical({"schema": 1, "context": context, "measurement": measurement, "outputs": {"0": output}})
        receipt_hash = audit.sha(raw_receipt)
        path = f"receipts/{receipt_hash}.json"
        self.put(path, raw_receipt)
        measurement["artifact_binding"] = {"schema": 1, "run_id": "fixture", "page": 0,
            "context_sha256": context_key, "receipt": path, "receipt_sha256": receipt_hash}
        return measurement

    def inspect(self, measurement):
        return self.auditor.attempt(self.case, "poppler", 0, ["pagura", "poppler"], "poppler", measurement)

    def test_valid_rgb_only_receipt_is_correct_with_stages_and_alpha_unavailable(self):
        actual = self.inspect(self.receipt())
        self.assertEqual(actual["classification"], "correct")
        self.assertTrue(actual["binding_valid"])
        self.assertEqual(actual["stage_status"], "unavailable")
        self.assertEqual(actual["alpha"]["status"], "unavailable")
        self.assertEqual(self.auditor.errors, [])

    def test_timeout_missing_output_remains_counted_with_elapsed_and_diagnostic(self):
        actual = self.inspect(self.receipt(status="timeout", missing=True))
        self.assertEqual(actual["classification"], "timeout")
        self.assertEqual(actual["rgb"]["status"], "missing-output")
        self.assertEqual(actual["metrics"]["elapsed_ns"], 100)
        self.assertEqual(actual["diagnostic"], "retained timeout tail")
        self.assertTrue(self.auditor.errors)
        self.assertEqual(len(self.auditor.attempts), 1)

    def test_nonzero_process_never_becomes_correct_from_a_partial_matching_image(self):
        actual = self.inspect(self.receipt(status="failed"))
        self.assertEqual(actual["rgb"]["status"], "correct")
        self.assertEqual(actual["classification"], "failed")

    def test_changed_raw_bytes_or_claimed_measurement_invalidates_binding(self):
        measurement = self.receipt()
        changed = copy.deepcopy(measurement)
        changed["elapsed_ns"] = 99
        self.assertIn("evidence_error", self.inspect(changed))
        self.auditor.seen.clear()
        raw_hash = measurement["output_sha256"]
        self.put(f"blobs/{raw_hash}.raw", b"P6\n1 1\n255\n\1\0\0")
        self.assertIn("evidence_error", self.inspect(measurement))

    def test_context_false_cannot_alias_page_zero(self):
        actual = self.inspect(self.receipt(context_page=False))
        self.assertIn("evidence_error", actual)
        self.assertFalse(actual["binding_valid"])

    def test_prelaunch_refusal_is_not_a_launched_or_disappearing_sample(self):
        actual = self.inspect({"status": "invalid-evidence", "exit_code": None, "artifact_error": "budget exhausted"})
        self.assertEqual(actual["classification"], "prelaunch-refused")
        self.assertEqual(len(self.auditor.attempts), 1)
        self.assertTrue(self.auditor.errors)


if __name__ == "__main__":
    unittest.main()
