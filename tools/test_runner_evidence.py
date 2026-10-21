"""Receipt consistency checks; synthetic records are not runner attestations."""
import copy
import hashlib
import json
import os
import tempfile
import unittest
import uuid
from pathlib import Path

from benchkit.config import ROOT
from benchkit.runner_evidence import coordination_pointer, read_bound_report, verify_runner_receipt


class RunnerEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name).resolve()
        self.receipt_path = self.path / "runner.json"
        self.report_path = self.path / "report.json"
        self.environment = {"PAGURA_RUN_ID": str(uuid.uuid4()),
                            "PAGURA_RUN_RECEIPT": str(self.receipt_path)}
        self.report = {"commit": "a" * 40, "source_sha256": "b" * 64,
                       "runner": {"controlled": False, "coordination": coordination_pointer(self.environment)}}
        self.report_path.write_text(json.dumps(self.report))
        self.receipt = {
            "schema": 1, "run_id": self.environment["PAGURA_RUN_ID"],
            "environment_overrides": self.environment,
            "kind": "benchmark", "status": "completed", "exit_code": 0, "wrapper_exit_code": 0,
            "cancellation_signal": None, "errors": [], "artifact_errors": {}, "source_unchanged": True,
            "duration_ns": 123, "child_pid": 42, "argv": ["python3", "tools/bench.py", "run"],
            "started_at": "2026-10-20T10:00:00+00:00", "ended_at": "2026-10-20T10:00:01+00:00",
            "finalized_at": "2026-10-20T10:00:02+00:00",
            "cwd": str(ROOT.resolve()), "source_root": str(ROOT.resolve()),
            "coordination": {"scope": "cooperative", "lock_file": "/tmp/pagura-bench.lock",
                             "lock_directory": "/tmp/pagura-bench.lockdir"},
            "source_before": {"commit": self.report["commit"], "sha256": self.report["source_sha256"]},
            "source_after": {"commit": self.report["commit"], "sha256": self.report["source_sha256"]},
            "artifacts": {str(self.report_path): hashlib.sha256(self.report_path.read_bytes()).hexdigest()},
        }

    def pin(self):
        self.receipt_path.write_text(json.dumps(self.receipt))
        return hashlib.sha256(self.receipt_path.read_bytes()).hexdigest()

    def verify(self, **changes):
        args = {"report": self.report, "report_path": self.report_path,
                "receipt_path": self.receipt_path, "receipt_sha256": self.pin(),
                "source_sha256": self.report["source_sha256"]}
        args.update(changes)
        return verify_runner_receipt(**args)

    def test_bound_completed_receipt_does_not_upgrade_controlled_runner(self):
        self.assertEqual(self.verify()["run_id"], self.environment["PAGURA_RUN_ID"])
        self.assertIs(self.report["runner"]["controlled"], False)
        self.assertNotIn("controlled", self.receipt)

    def test_no_runner_environment_is_optional_but_partial_environment_is_invalid(self):
        self.assertIsNone(coordination_pointer({}))
        for env in ({"PAGURA_RUN_ID": self.environment["PAGURA_RUN_ID"]},
                    {"PAGURA_RUN_RECEIPT": str(self.receipt_path)},
                    {**self.environment, "PAGURA_RUN_ID": "not-uuid"},
                    {**self.environment, "PAGURA_RUN_RECEIPT": "relative.json"}):
            with self.subTest(env=env), self.assertRaises(ValueError):
                coordination_pointer(env)

    def test_pin_must_be_independently_supplied_and_match_bytes(self):
        for overrides in ({"receipt_path": None}, {"report_path": None}, {"receipt_sha256": None},
                          {"receipt_sha256": "a" * 64}, {"receipt_sha256": "z" * 64}):
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                self.verify(**overrides)

    def test_rejects_nonterminal_failed_or_changed_source_receipts(self):
        cases = (("status", "running"), ("status", "canceled"), ("kind", "validation"),
                 ("exit_code", True), ("wrapper_exit_code", 1), ("cancellation_signal", 2),
                 ("source_unchanged", False), ("errors", ["failure"]), ("artifact_errors", {"x": "missing"}),
                 ("duration_ns", 0), ("child_pid", True), ("source_root", "/another/worktree"),
                 ("ended_at", "2026-10-20T09:00:00+00:00"), ("finalized_at", None))
        original = copy.deepcopy(self.receipt)
        for key, value in cases:
            with self.subTest(key=key):
                self.receipt = {**original, key: value}
                with self.assertRaises(ValueError):
                    self.verify()
        self.receipt = original
        for key in ("source_before", "source_after"):
            self.receipt = copy.deepcopy(original)
            self.receipt[key]["sha256"] = "c" * 64
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.verify()

    def test_receipt_cannot_bind_another_report(self):
        self.receipt["artifacts"] = {}
        with self.assertRaisesRegex(ValueError, "report bytes"):
            self.verify()

    def test_stale_loaded_report_object_is_rejected(self):
        self.report_path.write_text(json.dumps({**self.report, "fabricated": True}))
        with self.assertRaisesRegex(ValueError, "loaded report"):
            self.verify()

    def test_wrong_run_identity_is_rejected(self):
        self.report["runner"]["coordination"]["run_id"] = str(uuid.uuid4())
        with self.assertRaisesRegex(ValueError, "identities"):
            self.verify()

    def test_wrong_source_commit_is_rejected(self):
        self.receipt["source_after"]["commit"] = "c" * 40
        with self.assertRaisesRegex(ValueError, "source"):
            self.verify()

    def test_semantically_equal_report_bytes_are_still_hash_bound(self):
        self.report_path.write_text(json.dumps(self.report, indent=2))
        with self.assertRaisesRegex(ValueError, "report bytes"):
            self.verify()

    @unittest.skipUnless(hasattr(os, "mkfifo"), "requires POSIX FIFO")
    def test_receipt_fifo_is_rejected_without_blocking(self):
        self.pin()
        fifo = self.path / "fifo"
        os.mkfifo(fifo)
        with self.assertRaisesRegex(ValueError, "regular file"):
            self.verify(receipt_path=fifo)
        with self.assertRaisesRegex(ValueError, "regular file"):
            read_bound_report(fifo)

    def test_candidate_report_loader_requires_object(self):
        self.assertEqual(read_bound_report(self.report_path), self.report)
        self.report_path.write_text("[]")
        with self.assertRaisesRegex(ValueError, "object"):
            read_bound_report(self.report_path)


if __name__ == "__main__":
    unittest.main()
