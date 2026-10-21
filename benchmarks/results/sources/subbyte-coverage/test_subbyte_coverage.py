"""Pure protocol/oracle tests. No renderer or subprocess is invoked."""
from fractions import Fraction as F
import json
import re
import unittest

import subbyte_coverage as diagnostic


class OracleTests(unittest.TestCase):
    def test_three_models_have_independent_expected_answers(self):
        expected = {
            "single-tiny": (F(255), F(260865, 1024), F(260865, 1024)),
            "coincident-tiny": (F(255), 255 * F(1023, 1024)**1024, F(260865, 1024)),
            "disjoint-tiny": (F(255), 255 * F(1023, 1024)**1024, F(0)),
            "compound-tiny": (F(0), F(0), F(0)),
            "full-pixel": (F(0), F(0), F(0)),
            "disjoint-halves": (F(16129, 255), F(255, 4), F(0)),
        }
        for case, values in expected.items():
            refs = diagnostic.references(diagnostic.fixture_shapes(case))
            self.assertEqual(tuple(refs[name] for name in diagnostic.REFERENCE_NAMES), values)
        repeated = expected["coincident-tiny"][1]
        self.assertTrue(93 < repeated < 94)
        self.assertEqual(diagnostic.rational_record(repeated, reference=True)["integers_within_one_level"], [93, 94])

    def test_current_gate_is_not_replaced_by_research_or_rounded_reference(self):
        refs = diagnostic.references(diagnostic.fixture_shapes("disjoint-tiny"))
        result = diagnostic.evaluate(bytes([255, 255, 255, 255]), refs, 255)
        self.assertTrue(result["declared_gate_pass"])
        self.assertTrue(result["prediction_matches"])
        self.assertFalse(diagnostic.evaluate(bytes([94, 94, 94, 255]), refs, 255)["declared_gate_pass"])
        halves = diagnostic.references(diagnostic.fixture_shapes("disjoint-halves"))
        self.assertFalse(diagnostic.evaluate(bytes([62, 62, 62, 255]), halves, 63)["declared_gate_pass"])
        changed = diagnostic.evaluate(bytes([64, 64, 64, 255]), halves, 63)
        self.assertTrue(changed["declared_gate_pass"])
        self.assertFalse(changed["prediction_matches"])
        self.assertFalse(diagnostic.evaluate(bytes([63, 63, 63, 254]), halves, 63)["declared_gate_pass"])

    def test_pdfs_use_exact_coordinates_and_distinct_paint_grouping(self):
        counts = {"single-tiny": (2, 2), "coincident-tiny": (1025, 1025),
                  "disjoint-tiny": (1025, 1025), "compound-tiny": (1025, 2),
                  "full-pixel": (2, 2), "disjoint-halves": (3, 3)}
        for case, (rectangles, fills) in counts.items():
            raw = diagnostic.pdf_bytes(diagnostic.fixture_shapes(case))
            self.assertIn(b"/MediaBox [0 0 1 1]", raw)
            body = raw.split(b"stream\n", 1)[1].split(b"endstream", 1)[0]
            self.assertEqual(body.split().count(b"re"), rectangles)
            self.assertEqual(body.split().count(b"f"), fills)
            length = int(re.search(rb"/Length (\d+)", raw).group(1))
            self.assertEqual(length, len(body))
            xref = int(raw.split(b"startxref\n")[1].splitlines()[0])
            self.assertEqual(raw[xref:xref+5], b"xref\n")
            entries = raw[xref:].splitlines()[3:7]
            for number, entry in enumerate(entries, 1):
                offset = int(entry.split()[0])
                self.assertTrue(raw[offset:].startswith(f"{number} 0 obj\n".encode()))
        self.assertEqual(diagnostic.number(F(1, 1024)), "0.0009765625")
        self.assertEqual(diagnostic.number(F(1023, 1024)), "0.9990234375")
        with self.assertRaises(ValueError):
            diagnostic.number(F(1, 3))


class DriverRecordTests(unittest.TestCase):
    def record(self):
        return {"schema": 1, "accuracy": "exact", "operation": "full", "dpi": 72,
                "width": 1, "height": 1, "open_ns": 1, "compile_ns": 2,
                "render_ns": 3, "total_ns": 7, "metric_ns": 3}

    def test_counter_identity_and_complete_ascii_are_required(self):
        valid = json.dumps(self.record()) + "\n"
        self.assertEqual(diagnostic.parse_stages(valid, "exact"), self.record())
        for text in (valid + valid, valid + "\ufffd", "x" * diagnostic.LOG_BUDGET):
            with self.assertRaises(ValueError):
                diagnostic.parse_stages(text, "exact")
        for key, value in (("width", True), ("metric_ns", 4), ("total_ns", 5),
                           ("operation", "tile"), ("render_ns", 0), ("dpi", 72.0)):
            changed = {**self.record(), key: value}
            with self.assertRaises(ValueError):
                diagnostic.parse_stages(json.dumps(changed), "exact")


if __name__ == "__main__":
    unittest.main()
