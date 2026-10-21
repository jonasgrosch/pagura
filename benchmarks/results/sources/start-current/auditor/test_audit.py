"""Bounded auditor component tests; no renderer, producer or toolchain runs."""
import importlib.util
import json
from pathlib import Path
import types
import unittest

spec = importlib.util.spec_from_file_location("start_current_audit", Path(__file__).with_name("audit.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AuditTests(unittest.TestCase):
    def test_independent_matrix_has_exact_both_suite_counts(self):
        self.assertEqual(len(audit.OPAQUE_CELLS), 46)
        self.assertEqual(len(audit.CELLS), 84)
        self.assertEqual(len(set(audit.CELLS)), 84)
        self.assertEqual(len({(suite, case) for suite, case, _, _ in audit.CELLS}), 35)
        self.assertEqual(sum(cell[0] == "analytic19" for cell in audit.CELLS), 38)

    def test_schedule_preserves_every_phase_pair_and_balanced_order(self):
        preflight = audit.expected_schedule("preflight")
        self.assertEqual(len(preflight), 84)
        self.assertTrue(all(row["pair"] == 0 and row["order"] == "BC" for row in preflight))
        rows = audit.expected_schedule("confirmation")
        self.assertEqual(len(rows), 84 * 205)
        self.assertEqual(rows, audit.expected_schedule("confirmation"))
        self.assertEqual([r["sequence"] for r in rows], list(range(len(rows))))
        for index in range(84):
            for phase, count, bc in (("warmup", 5, 3), ("measured", 200, 100)):
                selected = [r for r in rows if r["cell_index"] == index and r["phase"] == phase]
                self.assertEqual({r["pair"] for r in selected}, set(range(count)))
                self.assertEqual(sum(r["order"] == "BC" for r in selected), bc)

    def test_rgb_and_alpha_are_independent_exact_checks(self):
        target = (1, 1, bytes([12, 34, 56]))
        header = b"RGBA8 1 1 premultiplied\n"
        self.assertEqual(audit.quality(header + bytes([12, 34, 56, 255]), target)["status"], "correct")
        alpha = audit.quality(header + bytes([12, 34, 56, 254]), target)
        self.assertTrue(alpha["rgb_equal"])
        self.assertFalse(alpha["opaque_alpha_equal"])
        self.assertEqual(alpha["status"], "incorrect")
        wrong = audit.quality(header + bytes([11, 34, 56, 255]), target)
        self.assertEqual((wrong["different_channels"], wrong["max_channel_error"]), (1, 1))

    def test_missing_and_malformed_output_are_quality_failures(self):
        target = (1, 1, bytes([0, 0, 0]))
        for raw in (None, b"not-rgba", b"RGBA8 2 1 premultiplied\n" + bytes(8),
                    b"RGBA8 1 1 premultiplied\n" + bytes(3)):
            with self.subTest(raw=raw):
                self.assertNotEqual(audit.quality(raw, target)["status"], "correct")

    def test_tile_reference_takes_top_left_rows_without_resampling(self):
        row = bytes(range(256)) + bytes(range(44))
        rgb = b"".join(bytes([value, value, value]) for value in row)
        raw = b"P6\n300 2\n255\n" + rgb + rgb[::-1]
        width, height, tile = audit.ppm(raw, "tile")
        self.assertEqual((width, height), (256, 2))
        self.assertEqual(tile, rgb[:768] + rgb[::-1][:768])

    def test_one_missing_value_suppresses_paired_metric_without_dropping(self):
        rows = [{"baseline": {"v": 10}, "candidate": {"v": 5}} for _ in range(200)]
        rows[73]["baseline"]["v"] = None
        result = audit.paired_metric(rows, lambda arm: arm["v"], None, False)
        self.assertEqual(result["status"], "unavailable-incomplete-pairs")
        self.assertEqual(result["available_values"], {"baseline": 199, "candidate": 200})

    def test_stats_keep_all_wrong_pixel_timings_and_fixed_bootstrap_arguments(self):
        rows = [{"baseline": {"v": i + 1, "quality": "incorrect"},
                 "candidate": {"v": 2 * (i + 1), "quality": "correct"}} for i in range(200)]
        calls = []
        module = types.SimpleNamespace(ratio_bounds=lambda *args: calls.append(args) or {"test": True})
        result = audit.paired_metric(rows, lambda arm: arm["v"], module, True)
        self.assertEqual(result["pairs"], 200)
        self.assertEqual(result["median_ratio"], 2)
        self.assertEqual(result["maximum_ratio"], 2)
        self.assertEqual(calls[0][2:], (5000, 0.95, 20261114))
        self.assertEqual(len(calls[0][0]), 200)
        self.assertEqual(result["status"], "descriptive-all-pairs")

    def test_boolean_timing_is_unavailable(self):
        rows = [{"baseline": {"v": 10}, "candidate": {"v": 5}} for _ in range(200)]
        rows[0]["candidate"]["v"] = True
        self.assertEqual(audit.paired_metric(rows, lambda arm: arm["v"], None, False)["status"],
                         "unavailable-incomplete-pairs")

    def test_journal_requires_every_arm_and_terminal_event(self):
        planned = audit.expected_schedule("preflight")[0]
        arm = {"command": ["binary"], "started_monotonic_ns": 3}
        row = {**planned, "started_monotonic_ns": 2, "finished_monotonic_ns": 8,
               "B": arm, "C": arm, "classification": "both-correct-equal", "raw_equal": True}
        report = {"runner": {}, "runner_source_before": {}, "producer_argv": [], "started_unix_ns": 1,
                  "records": [row], "status": "completed-preflight", "finished_unix_ns": 9,
                  "duration_ns": 8, "journal": {"events": 8}}
        events = [
            ("run-start", {"runner": {}, "source_before": {}, "producer_argv": [], "started_unix_ns": 1}),
            ("pair-start", {"pair_sequence": 0, "scheduled": {k: v for k, v in planned.items() if k != "sequence"},
                            "started_monotonic_ns": 2}),
            ("arm-start", {"pair_sequence": 0, "label": "B", **arm}),
            ("arm-result", {"pair_sequence": 0, "label": "B", "record": arm}),
            ("arm-start", {"pair_sequence": 0, "label": "C", **arm}),
            ("arm-result", {"pair_sequence": 0, "label": "C", "record": arm}),
            ("pair-finished", {"pair_sequence": 0, "classification": row["classification"], "raw_equal": True,
                               "finished_monotonic_ns": 8}),
            ("run-finished", {"status": report["status"], "finished_unix_ns": 9, "duration_ns": 8, "error": None})]
        encoded = [(json.dumps({"event": i, "type": kind, "monotonic_ns": i + 1, **fields}) + "\n").encode()
                   for i, (kind, fields) in enumerate(events)]
        audit.verify_journal(b"".join(encoded), report, [planned])
        for malformed in (encoded[:-1], encoded[:4] + encoded[5:], encoded + encoded[-1:]):
            with self.assertRaises(ValueError):
                audit.verify_journal(b"".join(malformed), report, [planned])

    def test_wrong_baseline_is_correction_cost_and_not_equivalent_output(self):
        rows = []
        for planned in audit.expected_schedule("preflight"):
            rows.append({**planned, "raw_equal": False,
                         "baseline": {"process_success": True, "quality": {"status": "incorrect"}},
                         "candidate": {"process_success": True, "quality": {"status": "correct"}}})
        summaries = audit.cell_summaries(rows, "preflight", None, None)
        self.assertEqual(len(summaries), 84)
        self.assertTrue(all(s["timing_interpretation"] == "correction-cost only" for s in summaries))
        self.assertFalse(any(s["equivalent_output_timing_qualified"] for s in summaries))


if __name__ == "__main__":
    unittest.main()
