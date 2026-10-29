"""Adapter controls over synthetic records and subprocess stubs; no engine imports."""
import contextlib
import copy
import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from capabilities import ROOT

spec = importlib.util.spec_from_file_location("capability_hayro", ROOT / "benchmarks/adapters/capability_hayro.py")
assert spec is not None and spec.loader is not None
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)

PAYLOAD = b"Synthetic attachment.\n"
HASH = "bb0530979076ce983311c32a3cbf23448fb488520e3af66529a30246dbf01680"


def envelope(records):
    return {"schema": 1, "status": "ok", "value": records}


class HayroAdapterTests(unittest.TestCase):
    def test_only_observed_bytes_are_hashed_and_duplicate_occurrences_survive(self):
        record = {"name": "../../never-open.txt", "bytes_hex": PAYLOAD.hex()}
        source = envelope([record, record])
        for operation in ("attachments", "annotation-attachments"):
            self.assertEqual(adapter.normalize(source, operation), envelope([
                {"name": record["name"], "sha256": HASH}, {"name": record["name"], "sha256": HASH}]))
            self.assertEqual(adapter.normalize(envelope([]), operation), envelope([]))
        self.assertEqual(source, envelope([record, record]), "input records remain unchanged")
        sorted_records = adapter.normalize(envelope([{"name": "z", "bytes_hex": ""}, {"name": "a", "bytes_hex": ""}]), "attachments")
        self.assertEqual([record["name"] for record in sorted_records["value"]], ["a", "z"])
        empty = adapter.normalize(envelope([{"name": "", "bytes_hex": ""}]), "attachments")
        self.assertEqual(empty["value"][0]["sha256"], "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")

    def test_invalid_record_shapes_and_encodings_do_not_become_empty_success(self):
        for record in ({}, {"name": True, "bytes_hex": "00"}, {"name": "\u00e9", "bytes_hex": "00"},
                       {"name": "f", "bytes_hex": "0"}, {"name": "f", "bytes_hex": "FF"},
                       {"name": "f", "bytes_hex": "00 01"}, {"name": "f", "bytes_hex": "gg"},
                       {"name": "f", "bytes_hex": None}, {"name": "f", "bytes_hex": "", "sha256": HASH}):
            with self.subTest(record=record), self.assertRaises(ValueError):
                adapter.normalize(envelope([record]), "attachments")
        for value in (None, [], {"schema": True, "status": "ok", "value": []},
                      {"schema": 1, "status": "unsupported", "reason": "unclassified"},
                      {"schema": 1, "status": "rejected", "reason": "", "value": []}):
            with self.assertRaises(ValueError):
                adapter.normalize(value, "attachments")

    def test_bounds_apply_to_count_names_and_aggregate_payloads(self):
        for records in ([{"name": "f", "bytes_hex": ""}] * 129,
                        [{"name": "f" * 1025, "bytes_hex": ""}],
                        [{"name": "f", "bytes_hex": "00" * 40000}] * 2):
            self.assertEqual(adapter.normalize(envelope(records), "attachments")["status"], "limit")
        records = [{"name": "f", "bytes_hex": "00" * 32768}] * 2
        self.assertEqual(adapter.normalize(envelope(records), "attachments")["status"], "ok")
        for status in ("limit", "adapter-error", "rejected"):
            value = {"schema": 1, "status": status, "reason": "fixed observation"}
            self.assertEqual(adapter.normalize(value, "attachments"), value)
        value = envelope({"Title": "Observed title"})
        self.assertEqual(adapter.normalize(copy.deepcopy(value), "metadata"), value)

    def test_subprocess_scope_timeout_and_failure_logs(self):
        for exception, expected in ((FileNotFoundError(), "engine-unavailable"),
                                    (subprocess.TimeoutExpired("stub", 15), "timeout")):
            with patch.object(adapter.subprocess, "run", side_effect=exception):
                self.assertEqual(adapter.execute("stub", ["attachments", "source.pdf"])[1]["status"], expected)
        def stub(command, *, stdout, stderr, timeout):
            self.assertEqual(command, ["stub", "annotation-attachments", "source.pdf"])
            self.assertEqual(timeout, 15)
            stderr.write(b"private source bytes must not enter reports")
            return subprocess.CompletedProcess(command, 2)
        with patch.object(adapter.subprocess, "run", side_effect=stub):
            _, error = adapter.execute("stub", ["annotation-attachments", "source.pdf"])
            self.assertEqual(error["status"], "adapter-error")
            self.assertIn("stderr SHA-256", error["reason"])
            self.assertNotIn("private source", json.dumps(error))
        def large(command, *, stdout, stderr, timeout):
            stdout.write(b" " * (adapter.MAX_TRANSFER + 1))
            return subprocess.CompletedProcess(command, 0)
        with patch.object(adapter.subprocess, "run", side_effect=large):
            self.assertEqual(adapter.execute("stub", ["attachments", "source.pdf"])[1]["status"], "limit")

    def test_version_and_observation_bridge_preserve_identity_and_fixed_output_paths(self):
        version = {"engine": "hayro 0.8.0", "wrapper": "hayro-syntax public API 0.8.0"}
        with tempfile.TemporaryDirectory() as directory:
            def stub(command, *, stdout, stderr, timeout):
                self.assertEqual(Path(stdout.name).name, "stdout")
                self.assertEqual(Path(stderr.name).name, "stderr")
                stdout.write(json.dumps(version if command[-1] == "--version" else envelope([
                    {"name": "../hostile.pdf", "bytes_hex": PAYLOAD.hex()}])).encode())
                return subprocess.CompletedProcess(command, 0)
            for arguments in (["stub", "--version"], ["stub", "attachments", str(Path(directory) / "source.pdf")]):
                out = io.StringIO()
                with patch.object(adapter.sys, "argv", ["adapter", *arguments]), patch.object(adapter.subprocess, "run", side_effect=stub), contextlib.redirect_stdout(out):
                    adapter.main()
                observed = json.loads(out.getvalue())
                self.assertEqual(observed, version if arguments[-1] == "--version" else envelope([{"name": "../hostile.pdf", "sha256": HASH}]))
            self.assertEqual(list(Path(directory).iterdir()), [], "PDF labels cannot create output files")
