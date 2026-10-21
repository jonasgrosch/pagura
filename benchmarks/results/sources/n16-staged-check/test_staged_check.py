"""Pure protocol tests only: no repository imports or child processes."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import signal
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("staged_check", Path(__file__).with_name("staged_check.py"))
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def fixture():
    source = {"commit": "a" * 40, "sha256": "b" * 64}
    plan = {"source": source, "commands": [{"argv": ["check", str(i)], "env": {"CARGO_BUILD_JOBS": "1"}} for i in range(14)]}
    attempts = []
    for i in range(14):
        receipt = {"status": "completed", "source_before": source, "source_after": source,
                   "source_unchanged": True, "exit_code": 0, "wrapper_exit_code": 0,
                   "errors": [], "artifact_errors": {}, "cancellation_signal": None, "run_id": str(i),
                   "started_at": f"2026-10-21T00:{i:02}:10+00:00", "ended_at": f"2026-10-21T00:{i:02}:20+00:00"}
        report = {"status": "command-passed", "source_before": source, "source_after": source,
                  "command": plan["commands"][i], "run_id": str(i), "log": {"path": "/attempt/stage/stage.log"},
                  "process": {"status": "failed", "exit_code": 0}, "receipt": "/attempt/receipt.json"}
        guard = {"status": "completed", "observation_only": False, "exit_code": 0, "child_exit_code": 0,
                 "events": [{"stage": "preflight", "matches": []}, {"stage": "postflight", "matches": []}]}
        attempts.append({"index": i, "receipt": receipt, "report": report, "guard": guard,
                         "missing_stage_paths": ["/attempt/stage/report.json", "/attempt/stage/stage.log"]})
    return plan, attempts


def cancelled_before_first(attempts):
    row = deepcopy(attempts[0])
    row["receipt"].update(status="failed", exit_code=143, wrapper_exit_code=143,
                          started_at="2026-10-21T00:00:00+00:00", ended_at="2026-10-21T00:00:05+00:00")
    row["guard"].update(status="invalid-external-contention", exit_code=143, child_exit_code=143,
                        events=[{"stage": "running", "matches": [{"reason": "cargo-compute"}]}])
    row["report"].update(status="cancelled", log=None)
    row["report"].pop("process")
    row["receipt"]["artifact_errors"] = {"/attempt/stage/stage.log": "not available"}
    return row


class ProtocolTests(unittest.TestCase):
    def test_exact_fourteen_and_provisional_process_status(self):
        plan, attempts = fixture()
        check.decide(plan, attempts)

    def test_missing_duplicate_reordered_and_drift(self):
        for kind in ("missing", "duplicate", "order", "source", "command", "boolean"):
            plan, attempts = fixture()
            if kind == "missing": attempts.pop()
            if kind == "duplicate": attempts.append(deepcopy(attempts[0]))
            if kind == "order": attempts[1]["receipt"]["started_at"] = "2026-10-21T00:00:15+00:00"
            if kind == "source": attempts[0]["receipt"]["source_after"] = {"sha256": "changed"}
            if kind == "command": attempts[0]["report"]["command"] = {"argv": ["subset"], "env": {}}
            if kind == "boolean": attempts[0]["report"]["process"]["exit_code"] = False
            with self.subTest(kind=kind), self.assertRaises(ValueError): check.decide(plan, attempts)

    def test_retained_foreign_and_manual_cancellation_retries(self):
        plan, attempts = fixture()
        interrupted = cancelled_before_first(attempts)
        check.decide(plan, [interrupted, *attempts])
        interrupted["guard"]["status"] = "cancelled"
        interrupted["receipt"].update(status="canceled", cancellation_signal=signal.SIGTERM)
        check.decide(plan, [interrupted, *attempts])

    def test_semantic_failure_and_unknown_cleanup_cannot_be_retried(self):
        for kind in ("assertion", "nonzero", "timeout", "cleanup", "artifact", "unmatched"):
            plan, attempts = fixture()
            interrupted = cancelled_before_first(attempts)
            if kind == "assertion": interrupted["report"]["status"] = "failed"
            if kind == "nonzero": interrupted["report"]["process"] = {"exit_code": 1}
            if kind == "timeout": interrupted["report"]["process"] = {"status": "timeout", "exit_code": -9}
            if kind == "cleanup": interrupted["receipt"].update(status="evidence-failed", errors=["cleanup unknown"])
            if kind == "artifact": interrupted["receipt"]["artifact_errors"]["/unexpected"] = "missing"
            if kind == "unmatched": interrupted["guard"]["events"] = []
            with self.subTest(kind=kind), self.assertRaises(ValueError): check.decide(plan, [interrupted, *attempts])

    def test_rejected_aggregate_preserves_earlier_bytes_and_no_overwrite(self):
        plan, attempts = fixture()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "earlier.json"
            check.write(path, {"retained": True}, new=True)
            before = path.read_bytes()
            with self.assertRaises(ValueError): check.decide(plan, attempts[:-1])
            with self.assertRaises(FileExistsError): check.write(path, {"replace": True}, new=True)
            self.assertEqual(path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
