"""Check paired-bootstrap arithmetic, not statistical coverage guarantees."""
from fractions import Fraction
import unittest
from unittest.mock import patch

from benchkit.statistics import comparison_statistics, evaluate_cell, ratio_bounds


class RatioBoundsTests(unittest.TestCase):
    def test_proportional_pairs_have_exact_bounds_in_both_directions(self):
        # Large variation within each series must not become uncertainty when
        # the same pair indices are used for numerator and denominator.
        baseline = [1, 5, 40, 1000]
        target = [2, 10, 80, 2000]
        for first, second, expected in [(target, baseline, 2), (baseline, target, 0.5)]:
            with self.subTest(expected=expected):
                result = ratio_bounds(first, second, resamples=31, seed=7)
                self.assertEqual(set(result), {
                    "median_ratio", "p95_ratio", "median_upper", "p95_upper",
                    "median_lower", "p95_lower",
                })
                self.assertTrue(all(value == expected for value in result.values()))

    def test_exhaustive_two_pair_distribution_and_reciprocal_case(self):
        # Each length-two index sequence has probability 1/4. Enumerating all
        # four gives a known bootstrap distribution without Monte Carlo noise.
        # Forward medians: [1/2, 2, 2, 3]; p95s: [1/2, 172/59, 172/59, 3].
        # The 25th/75th percentiles interpolate at positions 3/4 and 9/4.
        # Swapping series reciprocates each draw, not interpolated endpoints.
        cases = [
            ([1, 9], [2, 3], {
                "median_ratio": 2, "p95_ratio": Fraction(172, 59),
                "median_lower": Fraction(13, 8), "median_upper": Fraction(9, 4),
                "p95_lower": Fraction(1091, 472), "p95_upper": Fraction(693, 236),
            }),
            ([2, 3], [1, 9], {
                "median_ratio": Fraction(1, 2), "p95_ratio": Fraction(59, 172),
                "median_lower": Fraction(11, 24), "median_upper": Fraction(7, 8),
                "p95_lower": Fraction(703, 2064), "p95_upper": Fraction(521, 688),
            }),
        ]
        for target, baseline, expected in cases:
            with self.subTest(target=target), patch("benchkit.statistics.random.Random") as random_factory:
                draws = random_factory.return_value.choices
                draws.side_effect = [(0, 0), (0, 1), (1, 0), (1, 1)]
                actual = ratio_bounds(target, baseline, resamples=4, confidence=0.75, seed=19)
                random_factory.assert_called_once_with(19)
                self.assertEqual(draws.call_count, 4)
                for key, value in expected.items():
                    self.assertAlmostEqual(actual[key], float(value), places=14, msg=key)

    def test_seed_reproduces_all_bounds_without_mutating_samples(self):
        target, baseline = [5, 2, 11], [4, 8, 7]
        actual = ratio_bounds(target, baseline, resamples=23, seed=53)
        self.assertEqual(actual, ratio_bounds(target, baseline, resamples=23, seed=53))
        self.assertEqual(target, [5, 2, 11])
        self.assertEqual(baseline, [4, 8, 7])

    def test_positive_resample_count_and_open_confidence_range(self):
        # These are input-domain checks, not an endorsement of small samples
        # or one bootstrap draw for an acceptance decision.
        for confidence in (0.01, 0.5, 0.99):
            with self.subTest(confidence=confidence):
                result = ratio_bounds([2, 4], (1, 2), resamples=1, confidence=confidence)
                self.assertTrue(all(value == 2 for value in result.values()))

    def test_invalid_resample_counts_fail(self):
        for value in (0, -1, True, False, 1.0, 1.5, float("nan"), float("inf"), None, "4"):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "positive integer"):
                ratio_bounds([1, 2], [2, 3], resamples=value)

    def test_invalid_confidences_fail(self):
        for value in (0, 1, -0.1, 1.1, True, False, float("nan"),
                      float("inf"), -float("inf"), 10 ** 1000, None, "0.95"):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "confidence"):
                ratio_bounds([1, 2], [2, 3], resamples=1, confidence=value)

    def test_missing_or_unmatched_pairs_fail(self):
        for target, baseline in (([], []), ([1], [2]), ([1, 2], [2]), ([1], [2, 3])):
            with self.subTest(target=target, baseline=baseline), self.assertRaisesRegex(ValueError, "complete pairs"):
                ratio_bounds(target, baseline, resamples=1)

    def test_invalid_samples_fail_in_either_series(self):
        invalid = (0, -1, float("nan"), float("inf"), -float("inf"),
                   True, False, None, "1", 1j, 10 ** 1000)
        for value in invalid:
            for target, baseline in (([1, value], [2, 3]), ([1, 2], [3, value])):
                with self.subTest(value=value, target=target), self.assertRaisesRegex(ValueError, "finite and positive"):
                    ratio_bounds(target, baseline, resamples=1)

    def test_nonfinite_or_underflowed_derived_ratios_fail(self):
        # Finite input samples can still overflow median arithmetic/division or
        # underflow their ratio. None of those values is usable gate evidence.
        for target, baseline in (([1e308] * 2, [1e308] * 2),
                                 ([1e308] * 3, [1e-308] * 3),
                                 ([1e-308] * 3, [1e308] * 3)):
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, "derived ratios"):
                ratio_bounds(target, baseline, resamples=1)


class CellStatisticsValidationTests(unittest.TestCase):
    def setUp(self):
        self.policy = {
            "minimum_samples": 2, "bootstrap_resamples": 4, "confidence": 0.95,
            "statistics": ["median", "p95"], "latency_ratio_max": 0.8,
            "peak_rss_ratio_max": 1,
        }
        digest = "a" * 64
        self.cell = {
            "engine": "example", "status": "measured", "oracle_pixels_sha256": digest,
            "samples": [
                {"pair": i, "order": ["pagura", "example"],
                 "pagura": {"status": "ok", "exit_code": 0, "pixels_sha256": digest,
                            "elapsed_ns": 70, "peak_rss_bytes": 50},
                 "baseline": {"status": "ok", "exit_code": 0, "pixels_sha256": digest,
                              "elapsed_ns": 100, "peak_rss_bytes": 100}}
                for i in range(2)
            ],
        }

    def test_boolean_or_unrepresentable_timing_fails_without_statistics(self):
        for value in (True, False, 10 ** 1000):
            with self.subTest(value=value):
                self.cell["samples"][0]["pagura"]["elapsed_ns"] = value
                result = evaluate_cell(self.cell, self.policy)
                self.assertFalse(result["pass"])
                self.assertIn("invalid timing", result["reasons"])
                self.assertEqual(result["metrics"], {})

    def test_invalid_bootstrap_policy_fails_closed(self):
        for key, value in (("confidence", float("nan")), ("confidence", 1),
                           ("bootstrap_resamples", True), ("bootstrap_resamples", 0)):
            with self.subTest(key=key, value=value):
                result = evaluate_cell(self.cell, {**self.policy, key: value})
                self.assertFalse(result["pass"])
                self.assertTrue(any(reason.startswith("invalid statistics:") for reason in result["reasons"]))
                self.assertEqual(result["metrics"], {})

    def test_overflowed_derived_statistics_fail_closed(self):
        for sample in self.cell["samples"]:
            sample["pagura"]["elapsed_ns"] = 1e308
            sample["baseline"]["elapsed_ns"] = 1e308
        result = evaluate_cell(self.cell, self.policy)
        self.assertFalse(result["pass"])
        self.assertIn("invalid statistics: derived ratios must be finite and positive", result["reasons"])
        self.assertEqual(result["metrics"], {})

    def test_lower_bounds_do_not_replace_existing_upper_bound_gate(self):
        for timing, expected_pass in ((70, True), (81, False)):
            with self.subTest(timing=timing):
                for sample in self.cell["samples"]:
                    sample["pagura"]["elapsed_ns"] = timing
                result = evaluate_cell(self.cell, self.policy)
                self.assertEqual(result["pass"], expected_pass)
                self.assertIn("median_lower", result["metrics"])
                self.assertIn("p95_lower", result["metrics"])

    def test_invalid_rss_fails_gate_and_remains_missing_in_exploratory_metrics(self):
        for side in ("pagura", "baseline"):
            for value in (True, False, 0, -1, 50.0, None, "50", float("nan"), float("inf")):
                with self.subTest(side=side, value=value), \
                        patch.dict(self.cell["samples"][0][side], {"peak_rss_bytes": value}):
                    result = evaluate_cell(self.cell, self.policy)
                    self.assertFalse(result["pass"])
                    self.assertIn("missing peak RSS", result["reasons"])
                    self.assertEqual(result["metrics"], {})
                    exploratory = comparison_statistics(self.cell, self.policy)
                    self.assertEqual(exploratory["status"], "measured")
                    self.assertIsNone(exploratory["peak_rss_ratio"])

    def test_overflowed_or_underflowed_rss_ratio_cannot_be_reported_as_measured(self):
        for pagura_rss, baseline_rss in ((10 ** 1000, 1), (1, 10 ** 1000)):
            with self.subTest(pagura_rss=pagura_rss):
                for sample in self.cell["samples"]:
                    sample["pagura"]["peak_rss_bytes"] = pagura_rss
                    sample["baseline"]["peak_rss_bytes"] = baseline_rss
                result = evaluate_cell(self.cell, self.policy)
                self.assertFalse(result["pass"])
                self.assertIn("invalid statistics: derived ratios must be finite and positive", result["reasons"])
                self.assertEqual(result["metrics"], {})
                exploratory = comparison_statistics(self.cell, self.policy)
                self.assertEqual(exploratory["status"], "invalid-statistics")
                self.assertNotIn("median_ratio", exploratory)
                self.assertNotIn("peak_rss_ratio", exploratory)

    def test_exploratory_invalid_timing_statistics_are_explicit(self):
        self.cell["samples"][0]["pagura"]["elapsed_ns"] = 10 ** 1000
        result = comparison_statistics(self.cell, self.policy)
        self.assertEqual(result, {
            "status": "invalid-statistics", "reason": "samples must be finite and positive",
        })

    def test_exploratory_invalid_bootstrap_policy_is_explicit(self):
        for key, value in (("confidence", float("nan")), ("bootstrap_resamples", True)):
            with self.subTest(key=key):
                result = comparison_statistics(self.cell, {**self.policy, key: value})
                self.assertEqual(result["status"], "invalid-statistics")
                self.assertIn("reason", result)
                self.assertNotIn("median_ratio", result)


if __name__ == "__main__":
    unittest.main()
