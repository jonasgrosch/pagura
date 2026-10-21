#!/usr/bin/env python3
"""Small pure selection regressions; never invoke a renderer or benchmark loop."""
from types import SimpleNamespace
import unittest

from paired_render import selected_targets
from evaluate_n16_output import declared_cells
import rectangle_reserve as experiment


class TargetSelectionTests(unittest.TestCase):
    def setUp(self):
        self.fixture = SimpleNamespace(CASES=("primary", "secondary", "control"))
        self.fixture.CELLS = tuple((case, mode, operation) for case in self.fixture.CASES
                                  for mode in ("exact", "competitive") for operation in ("cold", "full"))

    def test_historical_single_target_and_operation_are_unchanged(self):
        actual = declared_cells(self.fixture, 3, 12, target_case="primary")
        expected = {cell: "target" if cell[0] == "primary" and cell[2] == "full" else "protected"
                    for cell in self.fixture.CELLS}
        self.assertEqual(actual, expected)
        self.assertEqual(selected_targets(self.fixture.CASES, "primary"), ("primary",))

    def test_explicit_targets_do_not_promote_other_operations(self):
        actual = declared_cells(self.fixture, 3, 12, target_case="primary", target_operation="cold",
                                target_cases=("primary", "secondary"))
        self.assertEqual(sum(role == "target" for role in actual.values()), 4)
        self.assertTrue(all(actual[(case, mode, "full")] == "protected"
                            for case in self.fixture.CASES for mode in ("exact", "competitive")))
        self.assertEqual(selected_targets(self.fixture.CASES, "primary", ("primary", "secondary")),
                         ("primary", "secondary"))

    def test_invalid_frozen_target_sets_fail(self):
        for targets in ((), ("primary", "primary"), ("primary", "missing"), ("secondary",), "primary", ["primary"]):
            with self.subTest(targets=targets), self.assertRaises(ValueError):
                declared_cells(self.fixture, 3, 12, target_case="primary", target_cases=targets)

    def test_rectangle_matrix_keeps_all_modes_and_protected_cases(self):
        declared = declared_cells(experiment, 19, 38, target_case=experiment.TARGET_CASE,
                                  target_operation=experiment.TARGET_OPERATION, target_cases=experiment.TARGET_CASES)
        self.assertEqual(len(declared), 38)
        self.assertEqual(sum(role == "target" for role in declared.values()), 4)
        self.assertEqual(sum(role == "protected" for role in declared.values()), 34)
        for case in experiment.TARGET_CASES:
            for mode in ("exact", "competitive"):
                self.assertEqual(declared[(case, mode, "cold")], "target")
        self.assertEqual(experiment.BASELINE_COMMIT, "52d8e7f41e2bfc4d38bff394adcb2f4a1783a8c2")
        self.assertEqual(experiment.PHASE_PLAN["confirmation"], {"pairs": 200, "warmups": 5, "seed": 20261112})


if __name__ == "__main__":
    unittest.main()
