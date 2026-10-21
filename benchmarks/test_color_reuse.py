"""Focused checks for the fixed selection and frozen-input reader; no renderer."""
import hashlib
from pathlib import Path
import tempfile
import unittest

import color_reuse as experiment
import color_reuse_fixtures as fixtures
from evaluate_n16_output import declared_cells, validate_guard


class ColorReuseExperimentTests(unittest.TestCase):
    def test_exact_matrix_and_target_roles(self):
        self.assertEqual(len(experiment.CASES), 19)
        self.assertEqual(len(set(experiment.CELLS)), 38)
        self.assertEqual(experiment.CELLS, tuple((name, mode, "cold")
            for name in fixtures.CASES for mode in ("exact", "competitive")))
        declared = declared_cells(experiment, 19, 38,
            target_case=experiment.TARGET_CASE, target_operation=experiment.TARGET_OPERATION)
        self.assertEqual({cell for cell, role in declared.items() if role == "target"},
            {("classic/many-operands", "exact", "cold"),
             ("classic/many-operands", "competitive", "cold")})
        self.assertEqual(sum(role == "protected" for role in declared.values()), 36)

    def test_every_fixture_reloads_frozen_external_pdf_and_oracle(self):
        for name in fixtures.CASES:
            with self.subTest(case=name):
                pdf, ppm = fixtures.fixture(name)
                self.assertTrue(pdf.startswith(b"%PDF-"))
                self.assertTrue(ppm.startswith(b"P6\n"))
        with self.assertRaisesRegex(ValueError, "unknown frozen"):
            fixtures.fixture("classic/missing")

    def test_frozen_reader_refuses_changed_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source"
            path.write_bytes(b"pinned")
            digest = hashlib.sha256(b"pinned").hexdigest()
            self.assertEqual(fixtures.pinned(path, digest), b"pinned")
            path.write_bytes(b"altered")
            with self.assertRaisesRegex(ValueError, "frozen analytic reference changed"):
                fixtures.pinned(path, digest)

    def test_reference_paths_cannot_escape_checkout(self):
        for path in ("../outside", "/absolute", "corpus/../outside"):
            with self.subTest(path=path), self.assertRaises(ValueError):
                fixtures.corpus_path(path)

    def test_strict_guard_rejects_incomplete_or_contended_observations(self):
        guard = {"status": "completed", "observation_only": False,
                 "exit_code": 0, "child_exit_code": 0,
                 "events": [{"stage": "preflight", "matches": []},
                            {"stage": "postflight", "matches": []}]}
        validate_guard(guard, "strict")
        for change in ({"child_exit_code": 1}, {"observation_only": True},
                       {"cancellation_requested_utc": "2026-10-21T00:00:00Z"},
                       {"events": [{"stage": "preflight", "matches": []}]},
                       {"events": guard["events"] + [{"stage": "running", "matches": [123]}]}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_guard({**guard, **change}, "strict")
        validate_guard({"status": "completed-observation-only", "exit_code": 0})

    def test_historical_matrix_defaults_still_require_original_counts(self):
        import n16_output_u16
        cells = declared_cells(n16_output_u16)
        self.assertEqual(len(cells), 46)
        self.assertEqual(sum(role == "target" for role in cells.values()), 2)
        with self.assertRaises(ValueError):
            declared_cells(experiment)


if __name__ == "__main__":
    unittest.main()
