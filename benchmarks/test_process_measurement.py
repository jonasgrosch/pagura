"""Offline tests for per-child accounting; no installed engines required."""
import json
import os
import signal
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from process_measurement import ROOT, measurement_options, run_process


@unittest.skipUnless(hasattr(os, "wait4"), "POSIX wait4 required")
class ProcessAccounting(unittest.TestCase):
    def test_user_system_units_and_parallelism_flag(self):
        child = SimpleNamespace(pid=123, returncode=None)
        usage = SimpleNamespace(ru_utime=0.75, ru_stime=0.4, ru_maxrss=12345)
        with patch("process_measurement.sys.platform", "darwin"), \
             patch("process_measurement.subprocess.Popen", return_value=child), \
             patch("process_measurement.os.wait4", return_value=(123, 0, usage)), \
             patch("process_measurement.time.perf_counter_ns", side_effect=[0, 0, 1_000_000_000]):
            result, _ = run_process(["unused"], {"timeout_seconds": 5})
        self.assertEqual(result["cpu_user_ns"], 750_000_000)
        self.assertEqual(result["cpu_system_ns"], 400_000_000)
        self.assertEqual(result["cpu_ns"], 1_150_000_000)
        self.assertEqual(result["cpu_wall_ratio"], 1.15)
        self.assertTrue(result["cpu_parallelism_flag"])
        self.assertEqual(result["exit_code"], 0)
        self.assertEqual(result["peak_rss_bytes"], 12345 * (1 if sys.platform == "darwin" else 1024))

    def test_launcher_keeps_directory_arguments_and_process_group(self):
        arguments = ["spaces and 'quotes'", '$(exit 42); "$HOME"']
        with patch("process_measurement.sys.platform", "linux"):
            result, log = run_process([sys.executable, "-c",
                'import json,os,sys;print(json.dumps([os.getcwd(),sys.argv[1:],os.getpid(),os.getpgrp(),os.getppid(),os.getsid(0)]));'
                'print("stderr retained",file=sys.stderr)', *arguments], {"timeout_seconds": 5})
        self.assertEqual(result["exit_code"], 0)
        directory, received, pid, group, parent, session = next(json.loads(line) for line in log.splitlines() if line.startswith("["))
        self.assertEqual(directory, str(ROOT))
        self.assertEqual(received, arguments)
        self.assertEqual(group, parent)
        self.assertEqual(group, session)
        self.assertNotEqual(pid, group)
        self.assertIn("stderr retained", log)

    @unittest.skipUnless(sys.platform == "linux", "Linux RSS inheritance regression")
    def test_linux_child_peaks_ignore_parent_and_preceding_children(self):
        # The supervisor must report its engine child's peak, never its own wait4 peak.
        parent_memory = bytearray(128 * 1024 * 1024)
        options = measurement_options()
        self.assertGreater(options["rss_floor_bytes"], 0)
        self.assertLess(options["rss_floor_bytes"], len(parent_memory) // 2)
        results = []
        for megabytes in (64, 1):
            result, _ = run_process([sys.executable, "-c",
                f"memory = bytearray({megabytes} * 1024 * 1024)"], {"timeout_seconds": 5})
            self.assertEqual(result["exit_code"], 0)
            results.append(result["peak_rss_bytes"])
        self.assertGreater(results[0], results[1] + 48 * 1024 * 1024)
        self.assertLess(results[1], len(parent_memory))

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
        for system in dict.fromkeys((sys.platform, "linux")):
            with self.subTest(platform=system), patch("process_measurement.sys.platform", system):
                result, _ = run_process([sys.executable, "-c", "import time;time.sleep(10)"],
                                        {"timeout_seconds": 0.05})
                self.assertEqual(result["status"], "timeout")
                self.assertLess(result["exit_code"], 0)
                missing, _ = run_process(["/pagura-no-such-command"], {"timeout_seconds": 1})
                self.assertIsNone(missing["cpu_ns"])
                self.assertEqual(missing["status"], "failed")

    def test_launcher_preserves_engine_exit_codes_and_signals(self):
        for expected, code in ((0, "pass"), (7, "raise SystemExit(7)"), (137, "raise SystemExit(137)"),
                               (-signal.SIGKILL, "import os,signal;os.kill(os.getpid(),signal.SIGKILL)"),
                               (-signal.SIGTERM, "import os,signal;os.kill(os.getpid(),signal.SIGTERM)")):
            with self.subTest(exit_code=expected), patch("process_measurement.sys.platform", "linux"):
                result, _ = run_process([sys.executable, "-c", code], {"timeout_seconds": 5})
            self.assertEqual(result["exit_code"], expected)
            self.assertEqual(result["rss_status"], "available")
            self.assertGreater(result["peak_rss_bytes"], 0)

    def test_launcher_preserves_unsupported_status(self):
        with patch("process_measurement.sys.platform", "linux"):
            result, _ = run_process([sys.executable, "-c",
                'print(\'{"status":"unsupported","feature":"test.feature"}\');raise SystemExit(3)'],
                {"timeout_seconds": 5})
        self.assertEqual(result["exit_code"], 3)
        self.assertEqual(result["status"], "unsupported")
        self.assertEqual(result["feature"], "test.feature")

    @unittest.skipUnless(sys.platform == "linux", "Linux process state required")
    def test_timeout_kills_engine_and_descendant(self):
        with tempfile.TemporaryDirectory() as directory:
            pids = Path(directory) / "pids.json"
            code = ('import json,os,subprocess,sys,time;'
                    'child=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"]);'
                    'open(sys.argv[1],"w").write(json.dumps([os.getpid(),child.pid]));time.sleep(30)')
            result, _ = run_process([sys.executable, "-c", code, str(pids)], {"timeout_seconds": 1})
            self.assertEqual(result["status"], "timeout")
            self.assertEqual(result["exit_code"], -signal.SIGKILL)
            self.assertIsNone(result["peak_rss_bytes"])
            self.assertEqual(result["rss_status"], "unavailable")
            self.assertEqual(result["cpu_status"], "unavailable")
            for pid in json.loads(pids.read_text()):
                for _ in range(100):
                    try:
                        state = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[0]
                    except (FileNotFoundError, ProcessLookupError):
                        state = None
                    if state in (None, "Z"):
                        break
                    time.sleep(0.01)
                try:
                    self.assertIn(state, (None, "Z"), f"engine process {pid} survived timeout")
                finally:
                    if state not in (None, "Z"):
                        os.kill(pid, signal.SIGKILL)

    def test_missing_launcher_accounting_fails_the_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            empty = Path(directory) / "empty.py"
            empty.write_text("pass\n")
            with patch("process_measurement.LINUX_LAUNCHER", empty), patch("process_measurement.sys.platform", "linux"):
                result, _ = run_process(["/bin/true"], {"timeout_seconds": 5})
        self.assertEqual(result["status"], "failed")
        self.assertIsNone(result["exit_code"])
        self.assertIsNone(result["peak_rss_bytes"])
        self.assertIn("Linux child accounting failed", result["diagnostic"])

    def test_launcher_timing_replaces_outer_timing_for_every_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            stub = Path(directory) / "stub.py"
            for exit_code in (0, 7, -signal.SIGTERM):
                stub.write_text('import os,sys\nos.write(int(sys.argv[1]), '
                                f'b"ok {exit_code} 1000000000 750000000 400000000 12345\\n")\n')
                with self.subTest(exit_code=exit_code), patch("process_measurement.LINUX_LAUNCHER", stub), \
                     patch("process_measurement.sys.platform", "linux"), \
                     patch("process_measurement.time.perf_counter_ns", side_effect=[0, 0, 200_000_000]):
                    result, _ = run_process(["unused"], {"timeout_seconds": 5})
                self.assertEqual(result["exit_code"], exit_code)
                self.assertEqual(result["elapsed_ns"], 1_000_000_000)
                self.assertEqual(result["cpu_user_ns"], 750_000_000)
                self.assertEqual(result["cpu_system_ns"], 400_000_000)
                self.assertEqual(result["cpu_ns"], 1_150_000_000)
                self.assertEqual(result["cpu_wall_ratio"], 1.15)
                self.assertTrue(result["cpu_parallelism_flag"])
                self.assertEqual(result["peak_rss_bytes"], 12345)

    def test_invalid_launcher_timing_fails_the_sample(self):
        with tempfile.TemporaryDirectory() as directory:
            stub = Path(directory) / "stub.py"
            for fields in ("ok 0 0 1 2 12345", "ok 0 100 -1 2 12345", "ok 0 100 1 2"):
                stub.write_text(f'import os,sys\nos.write(int(sys.argv[1]), b"{fields}\\n")\n')
                with self.subTest(record=fields), patch("process_measurement.LINUX_LAUNCHER", stub), \
                     patch("process_measurement.sys.platform", "linux"):
                    result, _ = run_process(["unused"], {"timeout_seconds": 5})
                self.assertEqual(result["status"], "failed")
                self.assertIsNone(result["exit_code"])
                self.assertEqual(result["cpu_status"], "unavailable")
                self.assertEqual(result["rss_status"], "unavailable")

    def test_timeout_keeps_outer_wall_time_even_with_an_engine_record(self):
        with tempfile.TemporaryDirectory() as directory:
            stub = Path(directory) / "stub.py"
            stub.write_text('import os,sys,time\n'
                            'os.write(int(sys.argv[1]), b"ok 0 1000000000 750000000 400000000 12345\\n")\n'
                            'os.write(1, b"ready\\n")\ntime.sleep(10)\n')
            with patch("process_measurement.LINUX_LAUNCHER", stub), patch("process_measurement.sys.platform", "linux"), \
                 patch("process_measurement.time.perf_counter_ns", side_effect=[0, 0, 2_000_000_000]):
                result, log = run_process(["unused"], {"timeout_seconds": 0.2})
        self.assertEqual(log, "ready\n")
        self.assertEqual(result["status"], "timeout")
        self.assertEqual(result["elapsed_ns"], 2_000_000_000)
        self.assertIsNone(result["cpu_ns"])
        self.assertIsNone(result["peak_rss_bytes"])

    def test_floor_is_measured_once_and_recorded(self):
        with patch("process_measurement.sys.platform", "linux"), patch("process_measurement.run_process",
                return_value=({"exit_code": 0, "rss_status": "available", "peak_rss_bytes": 12345}, "")) as run:
            options = measurement_options()
        run.assert_called_once_with(["/bin/true"], {"timeout_seconds": 10})
        self.assertEqual(options["rss_floor_bytes"], 12345)
        self.assertEqual(options["rss_floor_command"], ["/bin/true"])
        self.assertEqual(options["rss_launcher"][1:3], ["-I", "-S"])

    def test_interrupted_supervisor_kills_and_reaps_child(self):
        child = SimpleNamespace(pid=123, returncode=None)
        for system in ("darwin", "linux"):
            with self.subTest(platform=system), patch("process_measurement.sys.platform", system), \
                 patch("process_measurement.subprocess.Popen", return_value=child), \
                 patch("process_measurement.os.wait4", side_effect=[KeyboardInterrupt, (123, 9, None)]), \
                 patch("process_measurement.os.killpg") as kill:
                with self.assertRaises(KeyboardInterrupt):
                    run_process(["unused"], {"timeout_seconds": 5})
            kill.assert_called_once_with(123, signal.SIGKILL)
            self.assertEqual(child.returncode, -9)


if __name__ == "__main__":
    unittest.main()
