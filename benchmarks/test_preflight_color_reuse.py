"""Fixed-matrix preflight lifecycle checks; the renderer is never launched."""
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import preflight_color_reuse as preflight


class ColorPreflightLifecycleTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.output = self.directory / "evidence"
        bindings_path = self.directory / "bindings.json"
        identities = {}
        for side in ("baseline", "candidate"):
            binary = self.directory / side
            binary.write_bytes(("fake binary: " + side).encode())
            root = preflight.ROOT if side == "candidate" else self.directory / "control"
            identities[side] = {
                "root": str(root), "commit": "a" * 40, "source_sha256": "b" * 64,
                "binary": str(binary),
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            }
        harness = {"benchmarks/preflight_color_reuse.py": "c" * 64}
        bindings_path.write_text(json.dumps({
            "identities": identities, "bound_files_sha256": harness,
        }))
        self.expected_order = [
            (case, mode, operation, side)
            for case, mode, operation in preflight.experiment.CELLS
            for side in ("baseline", "candidate")
        ]
        self.assertEqual(len(preflight.experiment.CASES), 19)
        self.assertEqual(len(set(preflight.experiment.CELLS)), 38)
        self.assertEqual(len(self.expected_order), 76)
        source_by_root = {
            identity["root"]: {key: identity[key] for key in ("root", "commit", "source_sha256")}
            for identity in identities.values()
        }
        stack = ExitStack()
        self.addCleanup(stack.close)
        stack.enter_context(patch.object(preflight.sys, "argv", [
            "preflight_color_reuse.py", "--bindings", str(bindings_path),
            "--output", str(self.output),
        ]))
        stack.enter_context(patch.dict(preflight.os.environ, {
            "PAGURA_RUN_ID": "lifecycle-unit-test",
            "PAGURA_RUN_RECEIPT": str(self.directory / "outer-receipt.json"),
        }))
        stack.enter_context(patch.object(preflight.experiment, "require_baseline"))
        stack.enter_context(patch.object(preflight, "source_identity",
                                        side_effect=lambda root: source_by_root[str(root)]))
        stack.enter_context(patch.object(preflight, "harness_identity", return_value=harness))
        stack.enter_context(patch.object(preflight.experiment, "fixture", return_value=(
            b"%PDF-test-input\n", b"P6\n1 1\n255\n\xff\xff\xff",
        )))
        # Keep real bounded reads, hashing, normalization, image verification,
        # matrix iteration and evidence writing. Only external setup is faked.
        self.process = stack.enter_context(patch.object(preflight, "run_process"))
        stack.enter_context(patch("builtins.print"))

    def report(self):
        return json.loads((self.output / "report.json").read_text())

    def test_alternate_experiment_retains_all_strict_outputs_and_its_order(self):
        alternate = SimpleNamespace(
            CASES=tuple(reversed(preflight.experiment.CASES)),
            CELLS=tuple(reversed(preflight.experiment.CELLS)),
            __file__=__file__, require_baseline=Mock(),
            fixture=Mock(return_value=(b"%PDF-alternate\n", b"P6\n1 1\n255\n\xff\xff\xff")),
        )

        def renderer(command, _policy):
            Path(command[4]).write_bytes(b"RGBA8 1 1 premultiplied\n\xff\xff\xff\xff")
            return {"status": "failed", "exit_code": 0}, "renderer completed"

        self.process.side_effect = renderer
        self.assertEqual(preflight.main(experiment_module=alternate,
            experiment_name="rectangle-reserve", entrypoint=__file__), 0)
        alternate.require_baseline.assert_called_once()
        self.assertEqual(alternate.fixture.call_count, 19)
        self.assertEqual(self.process.call_count, 76)
        report = self.report()
        self.assertEqual(report["experiment"], "rectangle-reserve")
        self.assertEqual(report["status"], "passed-all-76")
        self.assertEqual(report["failures"], [])
        self.assertEqual(report["paired_output_differences"], [])
        expected = [(case, mode, operation, side)
                    for case, mode, operation in alternate.CELLS
                    for side in ("baseline", "candidate")]
        self.assertEqual([tuple(sample[key] for key in ("case", "mode", "operation", "side"))
                          for sample in report["samples"]], expected)
        self.assertTrue(all(sample["oracle_equal"] and sample["opaque_alpha_equal"]
                            for sample in report["samples"]))
        paths = preflight.harness_identity.call_args.args[0]
        self.assertIn(Path(preflight.experiment.__file__).resolve(), paths)
        self.assertIn(Path(__file__).resolve(), paths)

    def test_escaped_process_oserror_stops_after_the_first_attempt(self):
        self.process.side_effect = OSError("cleanup evidence unavailable")
        with self.assertRaisesRegex(OSError, "cleanup evidence unavailable"):
            preflight.main()
        self.process.assert_called_once()
        report = self.report()
        self.assertEqual(report["status"], "invalid-or-incomplete")
        self.assertFalse(report["admission"])
        self.assertEqual(report["expected_outputs"], 76)
        self.assertEqual(len(report["samples"]), 1)
        self.assertEqual(report["samples"][0]["status"], "failed")
        self.assertIn("OSError: cleanup evidence unavailable", report["fatal_error"])

    def test_returned_failure_and_pixel_mismatch_retain_the_whole_matrix(self):
        calls = 0

        def renderer(command, _policy):
            nonlocal calls
            calls += 1
            if calls == 1:
                return {"status": "failed", "exit_code": 2}, "ordinary renderer failure"
            pixels = b"\xfe\xff\xff\xff" if calls == 2 else b"\xff\xff\xff\xff"
            Path(command[4]).write_bytes(b"RGBA8 1 1 premultiplied\n" + pixels)
            # run_process intentionally returns this provisional status even
            # for exit0; actual pixel verification alone promotes the sample.
            return {"status": "failed", "exit_code": 0}, "renderer completed"

        self.process.side_effect = renderer
        self.assertEqual(preflight.main(), 1)
        self.assertEqual(self.process.call_count, 76)
        report = self.report()
        self.assertEqual(report["status"], "blocked-quality")
        self.assertFalse(report["admission"])
        self.assertNotIn("fatal_error", report)
        samples = report["samples"]
        self.assertEqual([
            tuple(sample[key] for key in ("case", "mode", "operation", "side"))
            for sample in samples
        ], self.expected_order)
        self.assertEqual(len(report["failures"]), 2)
        self.assertEqual(samples[0]["status"], "failed")
        self.assertEqual(samples[0]["process"]["exit_code"], 2)
        self.assertEqual(samples[0]["diagnostic"], "ordinary renderer failure")
        self.assertNotIn("output", samples[0])
        self.assertEqual(samples[1]["status"], "failed")
        self.assertEqual(samples[1]["max_channel_error"], 1)
        self.assertEqual(samples[1]["different_channels"], 1)
        self.assertFalse(samples[1]["oracle_equal"])
        self.assertTrue(samples[1]["opaque_alpha_equal"])
        self.assertTrue(all(sample["status"] == "passed" for sample in samples[2:]))
        self.assertEqual(sum("output" in sample for sample in samples), 75)
        for sample in samples[1:]:
            artifact = report["artifacts"][sample["output"]]
            raw = (self.output / sample["output"]).read_bytes()
            self.assertEqual(artifact["bytes"], len(raw))
            self.assertEqual(artifact["sha256"], hashlib.sha256(raw).hexdigest())
            self.assertEqual(sample["output_sha256"], artifact["sha256"])


if __name__ == "__main__":
    unittest.main()
