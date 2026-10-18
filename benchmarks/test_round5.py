"""Independent selection, parity, and accounting failure controls."""
import importlib.util
import json
import random
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from round5 import CONTENT, cpu_summary, failure_sink, parity, remaining_time, subset_300, verify_runtime_parity
from machine_record import probe


class MatrixContracts(unittest.TestCase):
    def candidates(self):
        cases, metadata, geometry = [], {}, {}
        for content in CONTENT:
            sizes = (["small", "small", "large"] + ["standard"] * 7 if content == "vector-heavy" else
                     ["small", "large", "standard", "standard", "standard"])
            for index, size in enumerate(sizes):
                identifier = f"{content}-{index}"
                for page in range(1 if content == "vector-heavy" else 2):
                    cases.append({"id": identifier, "page": page, "dpi": 72})
                    metadata[identifier, page] = {"content": content, "size": size, "producer": identifier}
                    geometry[identifier, page, 300] = (6000, 6000) if content == "vector-heavy" and size == "large" else (1000, 1000)
        return cases, metadata, geometry

    def test_300_selection_preserves_strata_and_diversity(self):
        cases, metadata, geometry = self.candidates()
        chosen = subset_300(cases, metadata, geometry, 17)
        self.assertEqual(len(chosen), 20)
        self.assertEqual(len({case["id"] for case in chosen}), 20)
        self.assertTrue(all(case["dpi"] == 300 for case in chosen))
        from collections import Counter
        selected = [metadata[case["id"], case["page"]] for case in chosen]
        self.assertEqual(Counter(item["content"] for item in selected), {name: 5 for name in CONTENT})
        self.assertEqual(Counter(item["size"] for item in selected), {"small": 4, "standard": 13, "large": 3})
        self.assertEqual(chosen, subset_300(cases, metadata, geometry, 17))

    def test_missing_stratum_is_not_silently_replaced(self):
        cases, metadata, geometry = self.candidates()
        cases = [case for case in cases if metadata[case["id"], case["page"]]["size"] != "large"]
        with self.assertRaisesRegex(ValueError, "cannot satisfy"):
            subset_300(cases, metadata, geometry, 17)

    def test_parity_requires_success_dimensions_and_exact_pixels(self):
        native = {"status": "unverified", "width": 2, "height": 3, "pixels_sha256": "a" * 64}
        self.assertTrue(parity(native, dict(native)))
        for change in ({"width": 3}, {"pixels_sha256": "b" * 64}, {"status": "failed"}):
            self.assertFalse(parity(native, {**native, **change}))

    def test_denied_machine_probe_is_explicit(self):
        with patch("machine_record.subprocess.run", side_effect=PermissionError("denied")):
            result = probe(["ps"])
        self.assertEqual(result["status"], "unavailable")
        self.assertIn("denied", result["diagnostic"])

    def test_runtime_parity_does_not_mask_an_engine_failure(self):
        good = {"status": "unverified", "width": 1, "height": 1, "pixels_sha256": "a" * 64}
        failed = {"status": "unsupported", "feature": "test.feature", "diagnostic": "precise error"}
        self.assertFalse(verify_runtime_parity(good, failed))
        self.assertEqual(failed["status"], "unsupported")
        changed = {**good, "pixels_sha256": "b" * 64}
        self.assertFalse(verify_runtime_parity(good, changed))
        self.assertEqual(changed["status"], "runtime-parity-mismatch")

    def test_deadline_stops_before_another_process(self):
        from types import SimpleNamespace
        with patch("round5.time.monotonic", return_value=10):
            self.assertEqual(remaining_time(SimpleNamespace(deadline=11)), 1)
            with self.assertRaisesRegex(RuntimeError, "time box exhausted"):
                remaining_time(SimpleNamespace(deadline=10))

    def test_failure_images_stay_inside_run_and_obey_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "bad.ppm"
            source.write_bytes(b"bad")
            sink = failure_sink(root / "run/failures")
            (root / "run").mkdir()
            result = {"status": "invalid-output"}
            sink(source, result, {"maximum_output_bytes": 3}, "untrusted/path")
            artifact = Path(result["artifact"])
            self.assertTrue(artifact.is_relative_to(root / "run"))
            self.assertEqual(artifact.read_bytes(), b"bad")
            source.write_bytes(b"oversize")
            refused = {"status": "invalid-output"}
            sink(source, refused, {"maximum_output_bytes": 3}, "other")
            self.assertNotIn("artifact", refused)
            self.assertEqual(len(list((root / "run/failures").iterdir())), 1)

    def test_failed_accounting_remains_visible_in_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            block = {"pair": 0, "runs": {"test": {"status": "failed", "cpu_ns": None, "peak_rss_bytes": None}}}
            (path / "blocks.jsonl").write_text(json.dumps(block) + "\n")
            result = cpu_summary(path)["test"]
        self.assertEqual(result["samples"], 1)
        self.assertEqual(result["accounted_samples"], 0)
        self.assertIsNone(result["median_cpu_wall"])
        self.assertEqual(result["statuses"], {"failed": 1})

    def test_warmup_parallelism_is_not_hidden(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            blocks = [{"pair": pair, "runs": {"test": {"status": "unverified", "cpu_wall_ratio": ratio,
                       "cpu_parallelism_flag": ratio > 1.05, "peak_rss_bytes": 100}}}
                      for pair, ratio in ((-1, 1.2), (0, 0.8))]
            (path / "blocks.jsonl").write_text("\n".join(map(json.dumps, blocks)) + "\n")
            result = cpu_summary(path)["test"]
        self.assertEqual(result["samples_above_1_05"], 0)
        self.assertEqual(result["warmup_samples_above_1_05"], 1)
        self.assertEqual(result["max_cpu_wall_including_warmups"], 1.2)

    @unittest.skipUnless(importlib.util.find_spec("PIL"), "Pillow only installed in isolated competitor environment")
    def test_fast_metrics_match_independent_scalar_reader(self):
        from agreement import pixel_difference
        from benchkit.images import pixel_difference as scalar
        rng = random.Random(17)
        for width, height in ((1, 1), (7, 11), (31, 5)):
            first = (width, height, bytes(rng.randrange(256) for _ in range(width * height * 3)))
            second = (width, height, bytes(rng.randrange(256) for _ in range(width * height * 3)))
            self.assertEqual(pixel_difference(first, second), scalar(first, second))
            self.assertEqual(pixel_difference(first, first), scalar(first, first))
        self.assertEqual(pixel_difference((1, 1, b"\0\0\0"), (2, 1, b"\0" * 6)),
                         scalar((1, 1, b"\0\0\0"), (2, 1, b"\0" * 6)))


if __name__ == "__main__":
    unittest.main()
