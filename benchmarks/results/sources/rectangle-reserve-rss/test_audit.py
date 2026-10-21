"""Bounded independent-auditor tests; no children, producer imports or rendering."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import audit


class ScheduleTests(unittest.TestCase):
    def test_fixed_matrix_order_and_rounds(self):
        rows = audit.schedule()
        self.assertEqual(len(rows), 2828)
        self.assertEqual(rows, audit.schedule())
        for offset in range(0, len(rows), 14):
            block = rows[offset:offset + 14]
            self.assertEqual({(p["binary_role"], p["case"], p["mode"]) for p in block}, set(audit.STRATA))
            self.assertEqual(len({(p["phase"], p["pair"]) for p in block}), 1)
        for role, name, mode in audit.STRATA:
            for phase, count in (("warmup", 2), ("measured", 200)):
                selected = [p for p in rows if (p["binary_role"], p["case"], p["mode"], p["phase"])
                            == (role, name, mode, phase)]
                self.assertEqual([p["pair"] for p in selected], list(range(count)))
                self.assertEqual(sum(p["order"] == "AB" for p in selected), count // 2)
                self.assertEqual(sum(p["order"] == "BA" for p in selected), count // 2)


class ReferenceTests(unittest.TestCase):
    def test_literal_rgb_channels_and_opaque_alpha(self):
        ppm = b"P6\n2 1\n255\n" + bytes([0, 128, 255, 255, 1, 2])
        self.assertEqual(audit.expected_rgba(ppm, 2, 1),
                         b"RGBA8 2 1 premultiplied\n" + bytes([0, 128, 255, 255, 255, 1, 2, 255]))

    def test_reference_payload_and_dimensions_fail_closed(self):
        ppm = b"P6\n1 1\n255\n\x01\x02\x03"
        for raw, width, height in ((ppm, 2, 1), (ppm[:-1], 1, 1), (ppm + b"\x00", 1, 1),
                                   (ppm.replace(b"255", b"65535"), 1, 1)):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                audit.expected_rgba(raw, width, height)


class LogTests(unittest.TestCase):
    def fixture(self):
        stage = {"schema": 1, "accuracy": "exact", "operation": "cold", "dpi": 72,
                 "width": 2, "height": 1, "open_ns": 10, "compile_ns": 30,
                 "render_ns": 60, "total_ns": 100, "metric_ns": 100}
        record = {"rss_bytes": 4096, "stages": stage, "process": {
            "status": "failed", "exit_code": 0, "elapsed_ns": 120,
            "rss_status": "available", "peak_rss_bytes": 4096}}
        raw = json.dumps(stage).encode() + b"\n 4096 maximum resident set size\n"
        return raw, record, {"width": 2, "height": 1}

    def test_raw_zero_exit_provisional_failed_is_verified_after_bytes(self):
        raw, record, image = self.fixture()
        audit.verify_log(raw, record, "exact", image)
        for key, value in (("exit_code", 1), ("exit_code", False), ("status", "timeout"),
                           ("peak_rss_bytes", 4096 * 1024), ("rss_status", "unavailable")):
            bad = copy.deepcopy(record)
            bad["process"][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                audit.verify_log(raw, bad, "exact", image)

    def test_ambiguous_rss_and_wrong_stage_cannot_pass(self):
        raw, record, image = self.fixture()
        for bad in (raw + b"1 maximum resident set size\n", raw + b'"1 maximum resident set size"\n',
                    raw.replace(b"4096 maximum", b"0 maximum"), raw + b"\xff", b"x" * (256 << 10)):
            with self.subTest(raw=bad[:80]), self.assertRaises((ValueError, UnicodeError)):
                audit.verify_log(bad, record, "exact", image)
        with self.assertRaises(ValueError):
            audit.verify_log(raw, record, "competitive", image)
        stage = {**record["stages"], "metric_ns": 60}
        record["stages"] = stage
        with self.assertRaises(ValueError):
            audit.verify_log(json.dumps(stage).encode() + b"\n4096 maximum resident set size\n", record, "exact", image)


class EvidenceTests(unittest.TestCase):
    def test_duplicate_nonfinite_and_wrong_external_pin_fail(self):
        for raw in (b'{"status":"failed","status":"completed"}', b'{"value":NaN}'):
            with self.assertRaises(ValueError):
                audit.decode(raw)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "evidence.json"
            path.write_bytes(b"{}")
            self.assertEqual(audit.bound({"path": str(path), "sha256": audit.sha(b"{}")}), b"{}")
            with self.assertRaises(ValueError):
                audit.bound({"path": str(path), "sha256": "0" * 64})
            with self.assertRaises(ValueError):
                audit.within(Path(directory), "../escape")

    def test_final_receipt_cannot_substitute_running_or_failed_snapshot(self):
        receipt = {"schema": 1, "status": "completed", "source_unchanged": True,
                   "source_before": {"commit": "x", "sha256": "y"}, "source_after": {"commit": "x", "sha256": "y"},
                   "errors": [], "artifact_errors": {}, "cancellation_signal": None,
                   "exit_code": 0, "wrapper_exit_code": 0,
                   "started_at": "2026-01-01T00:00:00+00:00", "ended_at": "2026-01-01T00:01:00+00:00",
                   "finalized_at": "2026-01-01T00:01:01+00:00"}
        audit.completed(receipt)
        for key, value in (("status", "running"), ("status", "failed"), ("exit_code", False),
                           ("wrapper_exit_code", 1), ("cancellation_signal", 15), ("artifact_errors", {"log": "missing"})):
            with self.subTest(key=key), self.assertRaises(ValueError):
                audit.completed({**receipt, key: value})


class SummaryTests(unittest.TestCase):
    def test_independent_quartiles_signed_order_differences_and_maxima(self):
        pairs = [{"order": "AB", "A": {"rss_bytes": 16}, "B": {"rss_bytes": 32}},
                 {"order": "BA", "A": {"rss_bytes": 64}, "B": {"rss_bytes": 48}}]
        actual = audit.summary(pairs)
        self.assertEqual(actual["rss_bytes_by_side"]["A"], {"count": 2, "values": [16, 64], "minimum": 16,
            "q1": 28, "median": 40, "q3": 52, "iqr": 24, "maximum": 64})
        self.assertEqual(actual["paired_B_minus_A_bytes"]["values"], [16, -16])
        self.assertEqual(actual["paired_second_minus_first_bytes"]["values"], [16, 16])
        self.assertEqual(actual["B_minus_A_bytes_by_order"]["BA"]["values"], [-16])
        self.assertEqual(actual["maximum_ratios"], {"B_over_A": .75, "A_over_B": 64 / 48})


if __name__ == "__main__":
    unittest.main()
