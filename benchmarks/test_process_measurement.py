"""Offline tests for per-child accounting; no installed engines required."""
import os
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from process_measurement import run_process


@unittest.skipUnless(hasattr(os, "wait4"), "POSIX wait4 required")
class ProcessAccounting(unittest.TestCase):
    def test_user_system_units_and_parallelism_flag(self):
        child = SimpleNamespace(pid=123, returncode=None)
        usage = SimpleNamespace(ru_utime=0.75, ru_stime=0.4, ru_maxrss=12345)
        with patch("process_measurement.subprocess.Popen", return_value=child), \
             patch("process_measurement.os.wait4", return_value=(123, 0, usage)), \
             patch("process_measurement.time.perf_counter_ns", side_effect=[0, 0, 1_000_000_000]):
            result, _ = run_process(["unused"], {"timeout_seconds": 5})
        self.assertEqual(result["cpu_user_ns"], 750_000_000)
        self.assertEqual(result["cpu_system_ns"], 400_000_000)
        self.assertEqual(result["cpu_ns"], 1_150_000_000)
        self.assertEqual(result["cpu_wall_ratio"], 1.15)
        self.assertTrue(result["cpu_parallelism_flag"])
        self.assertEqual(result["exit_code"], 0)

    def test_actual_child_and_unsupported_status(self):
        result, log = run_process([sys.executable, "-c",
            'print(\'{"status":"unsupported","feature":"test.feature"}\');raise SystemExit(3)'],
            {"timeout_seconds": 5})
        self.assertEqual(result["status"], "unsupported")
        self.assertEqual(result["feature"], "test.feature")
        self.assertGreater(result["cpu_ns"], 0)
        self.assertGreater(result["peak_rss_bytes"], 0)
        self.assertEqual(result["cpu_ns"], result["cpu_user_ns"] + result["cpu_system_ns"])
        self.assertIn("test.feature", log)

    def test_timeout_and_spawn_failure(self):
        result, _ = run_process([sys.executable, "-c", "import time;time.sleep(10)"],
                                {"timeout_seconds": 0.05})
        self.assertEqual(result["status"], "timeout")
        self.assertLess(result["exit_code"], 0)
        missing, _ = run_process(["/pagura-no-such-command"], {"timeout_seconds": 1})
        self.assertIsNone(missing["cpu_ns"])
        self.assertEqual(missing["status"], "failed")

    def test_interrupted_supervisor_kills_and_reaps_child(self):
        child = SimpleNamespace(pid=123, returncode=None)
        with patch("process_measurement.subprocess.Popen", return_value=child), \
             patch("process_measurement.os.wait4", side_effect=[KeyboardInterrupt, (123, 9, None)]), \
             patch("process_measurement.os.killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                run_process(["unused"], {"timeout_seconds": 5})
        kill.assert_called_once()
        self.assertEqual(child.returncode, -9)


if __name__ == "__main__":
    unittest.main()
