"""Bounded independent arithmetic/schema tests; no renderer or child process."""
import unittest

import audit


class ReferenceTests(unittest.TestCase):
    def test_fixed_count_and_root_rational_intervals(self):
        rows = audit.expected_requests()
        self.assertEqual(len(rows) * 2 * 2, 108)
        self.assertEqual(rows[0]["bounds"], [(218, 219)] * 3 + [(255, 255)])
        self.assertEqual(rows[2]["bounds"], [(5, 6)] * 3 + [(255, 255)])
        self.assertEqual(rows[1]["tile"], [0, 0, 1, 1])

    def test_declared_coverage_does_not_claim_continuous_scene(self):
        rows = audit.expected_requests()
        self.assertEqual(rows[6]["case"], "coverage-disjoint-tiny")
        self.assertEqual(rows[6]["bounds"], [(254, 255)] * 3 + [(255, 255)])
        self.assertEqual(rows[7]["case"], "coverage-compound-tiny")
        self.assertEqual(rows[7]["bounds"], [(0, 1)] * 3 + [(255, 255)])
        self.assertEqual(rows[9]["bounds"], [(63, 64)] * 3 + [(255, 255)])

    def test_background_alpha_and_coverage_are_distinct(self):
        rows = {row["case"]: row for row in audit.expected_requests()}
        covered = rows["background-128-half-coverage"]["bounds"]
        translucent = rows["background-128-half-opacity"]["bounds"]
        self.assertEqual(covered[:4], [(50, 50), (25, 25), (12, 12), (192, 192)])
        self.assertEqual(translucent[:4], [(50, 50), (25, 25), (13, 13), (192, 192)])
        self.assertEqual(rows["background-0-blank"]["bounds"], [(0, 0)] * 8)

    def test_colored_seam_uses_top_down_nonzero_tiles(self):
        rows = audit.expected_requests()
        full = rows[24]["bounds"]
        for index in (25, 26):
            x, y, width, height = rows[index]["tile"]
            crop = [channel for yy in range(y, y + height)
                    for channel in full[(yy * 259 + x) * 4:(yy * 259 + x + width) * 4]]
            self.assertEqual(rows[index]["bounds"], crop)
        self.assertEqual(rows[26]["bounds"][:4], [(241, 241), (25, 25), (13, 13), (255, 255)])

    def test_original_pdf_pin_cannot_be_replaced(self):
        with self.assertRaisesRegex(ValueError, "original PDF differs"):
            audit.check_pdf(b"%PDF-1.4\n", audit.expected_requests()[0])

    def test_literal_transparency_pdf_cannot_be_replaced(self):
        with self.assertRaisesRegex(ValueError, "literal independent PDF geometry changed"):
            audit.check_pdf(b"%PDF-1.4\n", audit.expected_requests()[10])


if __name__ == "__main__":
    unittest.main()
