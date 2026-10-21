"""Bounded pure/file tests; no child process, renderer or benchmark execution."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import rss_aa


class ScheduleTests(unittest.TestCase):
    def test_fixed_schedule_covers_all_strata_and_balances_order(self):
        rows = rss_aa.schedule()
        self.assertEqual(rows, rss_aa.schedule())
        self.assertEqual(len(rows), 2828)
        self.assertEqual(rss_aa.PROCESSES, 5656)
        self.assertEqual(len(rss_aa.CELLS), 7)
        self.assertEqual(len(set(rss_aa.STRATA)), 14)
        for start in range(0, len(rows), 14):
            block = rows[start:start + 14]
            self.assertEqual({(r["binary_role"], r["case"], r["mode"]) for r in block}, set(rss_aa.STRATA))
            self.assertEqual(len({(r["phase"], r["pair"]) for r in block}), 1)
            self.assertEqual({r["operation"] for r in block}, {"cold"})
        for role, case, mode in rss_aa.STRATA:
            for phase, count in (("warmup", 2), ("measured", 200)):
                selected = [r for r in rows if (r["binary_role"], r["case"], r["mode"], r["phase"])
                            == (role, case, mode, phase)]
                self.assertEqual([r["pair"] for r in selected], list(range(count)))
                self.assertEqual(sum(r["order"] == "AB" for r in selected), count // 2)
                self.assertEqual(sum(r["order"] == "BA" for r in selected), count // 2)


class RssTests(unittest.TestCase):
    def test_raw_darwin_units_and_unambiguous_complete_log(self):
        log = '{"schema":1}\n  0.1 real 0.1 user 0.0 sys\n  5341184  maximum resident set size\n'
        process = {"peak_rss_bytes": 5341184, "rss_status": "available"}
        self.assertEqual(rss_aa.rss_record(log, process), (log.encode("ascii"), 5341184))
        for bad in ("", log + "1 maximum resident set size\n", log + '"1 maximum resident set size"\n',
                    log.replace("5341184", "0"), log.replace("5341184", str(1 << 64)), log + "\ufffd",
                    "x" * rss_aa.LOG_BUDGET):
            with self.subTest(log=bad[:50]), self.assertRaises((ValueError, UnicodeError)):
                rss_aa.rss_record(bad, process)
        for value in (None, True, 5341184 * 1024):
            with self.subTest(value=value), self.assertRaises(ValueError):
                rss_aa.rss_record(log, {"peak_rss_bytes": value, "rss_status": "available"})
        with self.assertRaises(ValueError):
            rss_aa.rss_record(log, {"peak_rss_bytes": 5341184, "rss_status": "unavailable"})

    def test_descriptive_summary_keeps_extremes_order_and_partial_pairs(self):
        role, case, mode = rss_aa.STRATA[0]
        def pair(order, a, b, *, phase="measured", verified=True):
            return {"binary_role": role, "case": case, "mode": mode, "phase": phase, "order": order,
                    "A": {"verified": True, "rss_bytes": a}, "B": {"verified": verified, "rss_bytes": b}}
        rows = [pair("AB", 16, 32), pair("BA", 64, 48), pair("AB", 1000, 2000, phase="warmup"),
                pair("BA", 9000, 10000, verified=False)]
        result = rss_aa.summarize(rows)
        actual = result[rss_aa.key(role, case, mode)]
        self.assertEqual(actual["complete_measured_pairs"], 2)
        self.assertEqual(actual["rss_bytes_by_side"]["A"], {"count": 2, "values": [16, 64],
                         "minimum": 16, "q1": 28, "median": 40, "q3": 52, "iqr": 24, "maximum": 64})
        self.assertEqual(actual["paired_B_minus_A_bytes"]["values"], [16, -16])
        self.assertEqual(actual["paired_second_minus_first_bytes"]["values"], [16, 16])
        self.assertEqual(actual["rss_bytes_by_position"]["first"]["values"], [16, 48])
        self.assertEqual(actual["B_minus_A_bytes_by_order"]["BA"]["values"], [-16])
        self.assertEqual(actual["maximum_ratios"], {"B_over_A": .75, "A_over_B": 64 / 48})
        other = result[rss_aa.key(*rss_aa.STRATA[-1])]
        self.assertEqual(other["complete_measured_pairs"], 0)
        self.assertNotIn("maximum_ratios", other)
        self.assertEqual(len(rows), 4)
        self.assertFalse(rows[-1]["B"]["verified"])


class JournalTests(unittest.TestCase):
    def test_each_append_is_flushed_and_is_a_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            journal = rss_aa.Journal(path)
            try:
                record = {"verified": False, "error": "fixture failure"}
                journal.append("arm-start", pair_sequence=7, label="A", command=["fake"])
                self.assertEqual(len(path.read_bytes().splitlines()), 1)
                journal.append("arm-result", pair_sequence=7, label="A", record=record)
                record["verified"] = True
                rows = [json.loads(line) for line in path.read_bytes().splitlines()]
                self.assertEqual([row["event"] for row in rows], [0, 1])
                self.assertEqual(rows[1]["record"], {"verified": False, "error": "fixture failure"})
                self.assertEqual(journal.bytes, path.stat().st_size)
                self.assertEqual(journal.events, 2)
            finally:
                journal.close()
            with self.assertRaises(FileExistsError):
                rss_aa.Journal(path)

    def test_budget_failure_does_not_append_or_hide_nonfinite_data(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            journal = rss_aa.Journal(path)
            try:
                with patch.object(rss_aa, "MAX_EVENT", 8), self.assertRaises(ValueError):
                    journal.append("too-large", payload="value")
                with self.assertRaises(ValueError):
                    journal.append("invalid", payload=float("nan"))
                self.assertEqual(path.read_bytes(), b"")
                self.assertEqual((journal.bytes, journal.events), (0, 0))
            finally:
                journal.close()


if __name__ == "__main__":
    unittest.main()
