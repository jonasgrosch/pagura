"""Owned-group ordering and real Python children; no product builds or benchmarks."""
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from benchkit import measurement
from benchkit import process_group as groups


@unittest.skipUnless(hasattr(os, "WNOWAIT"), "requires POSIX waitid(WNOWAIT)")
class ProcessGroupTests(unittest.TestCase):
    def group(self):
        return groups.ProcessGroup(SimpleNamespace(pid=1234, wait=Mock(return_value=0)))

    def test_unreaped_anchor_precedes_snapshot_and_disarms_before_reap(self):
        group = self.group()
        events = []

        def waitid(*args):
            events.append("observe")
            group.child.wait.assert_not_called()
            self.assertEqual(args, (os.P_PID, 1234, os.WEXITED | os.WNOWAIT))
            return SimpleNamespace(si_pid=1234)

        def snapshot(*_args, **_kwargs):
            events.append("snapshot")
            group.child.wait.assert_not_called()
            return SimpleNamespace(stdout="1234 1234 Z\n5678 5678 S\n")

        def reap():
            events.append("reap")
            # A cancellation arriving during/following wait must never signal
            # the now-reusable numeric group. This also exercises reentrancy.
            group.forward(signal.SIGTERM)
            return 0

        group.child.wait.side_effect = reap
        with patch.object(groups.os, "waitid", side_effect=waitid), \
                patch.object(groups.subprocess, "run", side_effect=snapshot), \
                patch.object(groups.os, "killpg") as kill:
            group.wait_leader()
            self.assertFalse(group.drain())
            self.assertEqual(group.reap(), 0)
            group.forward(signal.SIGKILL)
            kill.assert_not_called()
        self.assertEqual(events, ["observe", "snapshot", "reap"])

    def test_signal_handler_can_reenter_lifecycle_lock(self):
        group = self.group()
        with patch.object(groups.os, "killpg") as kill, group._lock:
            group.forward(signal.SIGTERM)
        kill.assert_called_once_with(1234, signal.SIGTERM)

    def test_permission_error_persists_even_after_zero_exit(self):
        group = self.group()
        with patch.object(groups.os, "killpg", side_effect=PermissionError("denied")):
            group.forward(signal.SIGTERM)
        group.leader_exited = True
        self.assertEqual(group.reap(), 0)
        with self.assertRaisesRegex(groups.ProcessGroupError, "forwarding failed.*denied"):
            group.check()

    def test_failed_snapshot_is_not_an_empty_group(self):
        group = self.group()
        group.leader_exited = True
        for error in (PermissionError("denied"), subprocess.TimeoutExpired("ps", 5),
                      subprocess.CalledProcessError(1, "ps")):
            with self.subTest(error=error), patch.object(groups.subprocess, "run", side_effect=error), \
                    self.assertRaisesRegex(groups.ProcessGroupError, "cleanup is unverified"):
                group.drain()
        group.child.wait.assert_not_called()

    def test_missing_anchor_disarms_instead_of_signalling_reused_group(self):
        group = self.group()
        group.leader_exited = True
        with patch.object(groups.subprocess, "run", return_value=SimpleNamespace(stdout="5678 1234 S\n")), \
                patch.object(groups.os, "killpg") as kill:
            with self.assertRaisesRegex(groups.ProcessGroupError, "leader missing"):
                group.drain()
            group.forward(signal.SIGTERM)
            kill.assert_not_called()

    def test_external_reap_loses_ownership_and_never_signals(self):
        group = self.group()
        with patch.object(groups.os, "waitid", side_effect=ChildProcessError("reaped")), \
                patch.object(groups.os, "killpg") as kill:
            with self.assertRaisesRegex(groups.ProcessGroupError, "reaped outside"):
                group.wait_leader()
            group.forward(signal.SIGKILL)
            kill.assert_not_called()

    def test_live_children_drain_but_zombies_do_not_block(self):
        group = self.group()
        group.leader_exited = True
        snapshots = [SimpleNamespace(stdout="1234 1234 Z\n1235 1234 S\n"),
                     SimpleNamespace(stdout="1234 1234 Z\n1235 1234 Z+\n")]
        with patch.object(groups.subprocess, "run", side_effect=snapshots), \
                patch.object(groups.time, "sleep") as sleep:
            self.assertTrue(group.drain())
        sleep.assert_called_once()
        group.child.wait.assert_not_called()


@unittest.skipUnless(hasattr(os, "WNOWAIT"), "requires POSIX waitid(WNOWAIT)")
class MeasurementGroupTests(unittest.TestCase):
    def run_python(self, code, timeout=3):
        return measurement.run_process([sys.executable, "-c", code],
                                       {"timeout_seconds": timeout, "measure_rss": False})

    def test_normal_exit_has_no_post_reap_probe_or_signal(self):
        result, diagnostic = self.run_python("print('owned child completed')")
        self.assertEqual(result["exit_code"], 0)
        self.assertNotEqual(result["status"], "timeout")
        self.assertNotIn("timing_note", result)
        self.assertIn("owned child completed", diagnostic)

    def test_orphan_worker_finishes_before_measurement_returns(self):
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / "worker-finished"
            worker = f"import time; from pathlib import Path; time.sleep(.15); Path({str(marker)!r}).write_text('done')"
            parent = f"import subprocess, sys; subprocess.Popen([sys.executable, '-c', {worker!r}])"
            result, _ = self.run_python(parent)
            self.assertEqual(result["exit_code"], 0)
            self.assertEqual(marker.read_text(), "done")
            self.assertIn("owned process-group drain", result["timing_note"])

    def test_timeout_kills_original_group_and_restores_handlers(self):
        previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
        result, _ = self.run_python("import time; time.sleep(5)", timeout=.1)
        self.assertEqual(result["status"], "timeout")
        self.assertEqual(result["exit_code"], -signal.SIGKILL)
        self.assertEqual(previous, {sig: signal.getsignal(sig) for sig in previous})

    def test_missing_waitid_support_refuses_before_spawn(self):
        with patch.object(measurement, "require_group_support", side_effect=groups.ProcessGroupError("requires waitid")), \
                patch.object(measurement.subprocess, "Popen") as spawn, \
                self.assertRaisesRegex(groups.ProcessGroupError, "requires waitid"):
            self.run_python("pass")
        spawn.assert_not_called()

    def test_snapshot_failure_raises_before_any_output_verification(self):
        with patch.object(groups.ProcessGroup, "live_members", side_effect=groups.ProcessGroupError("snapshot denied")), \
                self.assertRaisesRegex(groups.ProcessGroupError, "snapshot denied"):
            self.run_python("pass")

    def test_watchdog_is_joined_before_group_reap(self):
        events = []
        timer = Mock()
        timer.join.side_effect = lambda: events.append("joined")
        original = groups.ProcessGroup.reap

        def reap(group):
            events.append("reaped")
            return original(group)

        with patch.object(measurement.threading, "Timer", return_value=timer), \
                patch.object(groups.ProcessGroup, "reap", reap):
            result, _ = self.run_python("pass")
        self.assertEqual(result["exit_code"], 0)
        self.assertEqual(events, ["joined", "reaped"])


if __name__ == "__main__":
    unittest.main()
