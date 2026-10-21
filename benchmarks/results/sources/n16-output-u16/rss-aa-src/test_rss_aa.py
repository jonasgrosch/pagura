"""Focused pure checks; these tests launch no renderer processes."""
import unittest
import rss_aa


class DiagnosticTests(unittest.TestCase):
    def test_fixed_balanced_schedule_interleaves_every_round(self):
        plan = rss_aa.schedule()
        self.assertEqual(len(plan), 42)
        self.assertEqual(plan, rss_aa.schedule())
        self.assertEqual(sum(len(row["order"]) for row in plan), 84)
        for start in range(0, len(plan), 3):
            round_rows = plan[start:start + 3]
            self.assertEqual({r["case"] for r in round_rows}, set(rss_aa.CASES))
            self.assertEqual(len({(r["phase"], r["pair"]) for r in round_rows}), 1)
        for case in rss_aa.CASES:
            for phase, count in (("warmup", 2), ("measured", 12)):
                orders = [r["order"] for r in plan if r["case"] == case and r["phase"] == phase]
                self.assertEqual(orders.count("AB"), count // 2)
                self.assertEqual(orders.count("BA"), count // 2)

    def test_rss_is_darwin_bytes_and_preserves_returned_ascii(self):
        log = '{"schema": 1}\n  1.00 real 0.50 user 0.25 sys\n  13615104  maximum resident set size\n'
        process = {"peak_rss_bytes": 13615104, "rss_status": "available"}
        self.assertEqual(rss_aa.rss_record(log, process), (log.encode("ascii"), 13615104))
        for invalid in ("", log + "1 maximum resident set size\n",
                        log + '"1 maximum resident set size"\n', log.replace("13615104", "0"),
                        log + "\ufffd", "x" * rss_aa.LOG_BUDGET):
            with self.subTest(invalid=invalid[:60]), self.assertRaises((ValueError, UnicodeError)):
                rss_aa.rss_record(invalid, process)
        for value in (None, True, 13615104 * 1024):
            with self.subTest(value=value), self.assertRaises(ValueError):
                rss_aa.rss_record(log, {"peak_rss_bytes": value, "rss_status": "available"})

    def test_summary_excludes_warmups_and_incomplete_pairs_and_keeps_order(self):
        records = [
            {"case": "opaque64", "phase": "measured", "order": "AB",
             "A": {"verified": True, "rss_bytes": 100}, "B": {"verified": True, "rss_bytes": 120}},
            {"case": "opaque64", "phase": "measured", "order": "BA",
             "A": {"verified": True, "rss_bytes": 150}, "B": {"verified": True, "rss_bytes": 90}},
            {"case": "opaque64", "phase": "warmup", "order": "AB",
             "A": {"verified": True, "rss_bytes": 999}, "B": {"verified": True, "rss_bytes": 999}},
            {"case": "opaque64", "phase": "measured", "order": "AB",
             "A": {"verified": True, "rss_bytes": 888}},
        ]
        result = rss_aa.summarize(records)["opaque64"]
        self.assertEqual(result["complete_measured_pairs"], 2)
        self.assertEqual(result["paired_B_minus_A_bytes"]["values"], [20, -60])
        self.assertEqual(result["paired_second_minus_first_bytes"]["values"], [20, 60])
        self.assertEqual(result["maximum_ratios"], {"B_over_A": 0.8, "A_over_B": 1.25})
        self.assertEqual(result["B_minus_A_bytes_by_order"]["BA"]["values"], [-60])


if __name__ == "__main__":
    unittest.main()
