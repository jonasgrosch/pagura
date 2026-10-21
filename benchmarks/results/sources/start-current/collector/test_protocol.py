"""Small protocol checks; no renderer execution or fixture imports."""
import unittest

from protocol import classification, opaque_bytes, quality, schedule


class ProtocolTests(unittest.TestCase):
    def test_fixed_balanced_schedule_keeps_every_cell(self):
        matrix = [{"suite": "test", "case": str(i), "mode": "exact", "operation": "cold"} for i in range(84)]
        planned = schedule(matrix, "confirmation")
        self.assertEqual(len(planned), 84 * 205)
        self.assertEqual(planned, schedule(matrix, "confirmation"))
        for phase, count, bc in (("warmup", 5, 3), ("measured", 200, 100)):
            for index in range(84):
                rows = [r for r in planned if r["phase"] == phase and r["cell_index"] == index]
                self.assertEqual([r["pair"] for r in rows], list(range(count)))
                self.assertEqual(sum(r["order"] == "BC" for r in rows), bc)
            for pair in range(count):
                rows = [r for r in planned if r["phase"] == phase and r["pair"] == pair]
                self.assertEqual(sorted(r["cell_index"] for r in rows), list(range(84)))
        preflight = schedule(matrix, "preflight")
        self.assertEqual(len(preflight), 84)
        self.assertTrue(all(r["order"] == "BC" and r["phase"] == "preflight" for r in preflight))

    def test_independent_rgb_and_alpha_contract(self):
        expected = (2, 1, bytes([0, 64, 219, 255, 191, 5]))
        raw = b"RGBA8 2 1 premultiplied\n" + bytes([0, 64, 219, 255, 255, 191, 5, 255])
        self.assertEqual(opaque_bytes(expected), raw)
        self.assertTrue(quality(raw, expected)["correct"])
        wrong = raw[:-2] + bytes([4, 254])
        result = quality(wrong, expected)
        self.assertFalse(result["correct"])
        self.assertEqual(result["rgb_differing_channels"], 1)
        self.assertEqual(result["rgb_maximum_error"], 1)
        self.assertEqual(result["nonopaque_pixels"], 1)
        for malformed in (raw[:-1], raw + b"x", raw.replace(b"2 1", b"1 2")):
            self.assertFalse(quality(malformed, expected)["correct"])

    def test_wrong_baseline_is_not_equal_work(self):
        def arm(correct, pin="same"):
            return {"process_success": True, "quality": {"correct": correct}, "output": {"sha256": pin}}
        self.assertEqual(classification({"B": arm(False), "C": arm(True)}), "baseline-incorrect-current-correct")
        self.assertEqual(classification({"B": arm(True), "C": arm(False)}), "baseline-correct-current-incorrect")
        self.assertEqual(classification({"B": arm(False), "C": arm(False)}), "neither-correct")
        self.assertEqual(classification({"B": arm(True), "C": arm(True)}), "both-correct-equal")
        self.assertEqual(classification({"B": arm(True), "C": arm(True, "other")}), "both-correct-bytes-differ")
        self.assertEqual(classification({"B": {"process_success": False}, "C": arm(True)}), "process-failure")


if __name__ == "__main__":
    unittest.main()
