"""Subject adapter controls; subprocesses are stubs, never local PDF engines."""
import importlib.util
import json
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from capabilities import ROOT

SPEC = importlib.util.spec_from_file_location("capability_pagura", ROOT / "benchmarks/adapters/capability_pagura.py")
assert SPEC is not None and SPEC.loader is not None
subject = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(subject)


class PaguraAdapterTests(unittest.TestCase):
    def test_page_dimensions_use_crop_rotation_and_user_unit_and_reject_bools(self):
        page = {"crop_box": [10, -20, 210, 80], "rotation": 90, "user_unit": 2}
        self.assertEqual(subject.page_sizes({"pages": 1, "page_geometry": [page]}),
                         {"count": 1, "sizes": [[200, 400]]})
        for change in ({"rotation": True}, {"user_unit": False}, {"crop_box": [0, 0, float("inf"), 1]},
                       {"crop_box": [0, 0, -1, 1]}, {"rotation": 45}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                subject.page_sizes({"pages": 1, "page_geometry": [{**page, **change}]})
        for count in (True, 2, -1):
            with self.assertRaises(ValueError):
                subject.page_sizes({"pages": count, "page_geometry": [page]})

    def test_metadata_absence_is_normalized_but_wrong_fields_are_not_discarded(self):
        self.assertEqual(subject.metadata({"info": None}), {"Title": "", "Author": ""})
        self.assertEqual(subject.metadata({"info": {"Title": "Grüße", "Author": None}}),
                         {"Title": "Grüße", "Author": ""})
        for bad in ({}, {"info": []}, {"info": {"Title": 42, "Author": ""}}, {"info": {}}):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                subject.metadata(bad)

    def test_only_known_exit_one_engine_messages_classify_refusals(self):
        for code, message, status in [
            (1, b"pagura: syntax error: invalid header at /private/path", "rejected"),
            (1, b"pagura: unsupported [filter.DCTDecode]: process", "unsupported"),
            (1, b"pagura: limit exceeded: allocation", "limit"),
            (-11, b"pagura: syntax error: crashed", "adapter-error"),
            (2, b"pagura: syntax error: usage", "adapter-error"),
            (1, b"unknown or IO failure", "adapter-error"),
        ]:
            response = subject.failure(code, message).response
            self.assertEqual(response["status"], status)
            self.assertNotIn("/private/path", str(response))
            if status == "unsupported":
                self.assertEqual(response["feature"], "filter.DCTDecode")

    def test_cli_output_is_bounded_and_timeout_and_missing_binary_are_distinct(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            def oversized(command, *, stdout, stderr, timeout):
                stdout.write(b"x" * (subject.MAX_OUTPUT + 1))
                return subprocess.CompletedProcess(command, 0)
            with patch.object(subject.subprocess, "run", side_effect=oversized):
                with self.assertRaises(subject.ProbeFailure) as raised:
                    subject.execute("stub", [], directory, time.monotonic() + 20)
                self.assertEqual(raised.exception.response["status"], "limit")
            for error, status in [(FileNotFoundError(), "engine-unavailable"),
                                  (subprocess.TimeoutExpired("stub", 20), "timeout")]:
                with patch.object(subject.subprocess, "run", side_effect=error):
                    with self.assertRaises(subject.ProbeFailure) as raised:
                        subject.execute("stub", [], directory, time.monotonic() + 20)
                    self.assertEqual(raised.exception.response["status"], status)

    def test_attachment_choice_and_hash_use_fresh_explicit_output_not_a_pdf_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            record = {"index": 0, "names": {"UF": "../../outside.txt", "F_hex": "6c6567616379"},
                      "payloads": [{"key": "F"}, {"key": "UF"}]}
            def extracted(binary, arguments, folder, deadline):
                self.assertEqual(arguments[:4], ["extract-attachment", "input.pdf", "0", "UF"])
                self.assertEqual(Path(arguments[4]), directory / "attachment.bin")
                self.assertFalse(Path(arguments[4]).exists())
                Path(arguments[4]).write_bytes(b"Synthetic attachment.\n")
                return b""
            with patch.object(subject, "execute", side_effect=extracted):
                actual = subject.attachments("stub", {"files": [record]}, "input.pdf", directory, time.monotonic() + 20)
            self.assertEqual(actual, [{"name": "../../outside.txt", "sha256": "bb0530979076ce983311c32a3cbf23448fb488520e3af66529a30246dbf01680"}])
            # A previous successful payload must not pass a later broken CLI.
            with patch.object(subject, "execute", return_value=b""):
                with self.assertRaisesRegex(ValueError, "did not create"):
                    subject.attachments("stub", {"files": [record]}, "input.pdf", directory, time.monotonic() + 20)
            record["payloads"] = [{"key": "F"}, {"key": "F"}]
            with self.assertRaisesRegex(ValueError, "duplicate"):
                subject.attachments("stub", {"files": [record]}, "input.pdf", directory, time.monotonic() + 20)

    def test_wrapping_preserves_empty_text_and_rejects_invalid_report_schemas(self):
        with patch.object(subject, "execute", return_value=b"\r\n"):
            self.assertEqual(subject.probe("stub", "text", "input.pdf"), {"schema": 1, "status": "ok", "value": ""})
        for value in ({"schema": True}, {"schema": 99}, {"schema": 1, "info": []}):
            with patch.object(subject, "execute", return_value=json.dumps(value).encode()):
                self.assertEqual(subject.probe("stub", "metadata", "input.pdf")["status"], "adapter-error")


if __name__ == "__main__":
    unittest.main()
