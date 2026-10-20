"""Runner coordination and receipt tests; no product toolchain or benchmark jobs."""
import contextlib
import fcntl
import hashlib
import io
import json
import os
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import runner


class LeaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.file = self.root / "lock"
        self.directory = self.root / "lockdir"

    def lease(self, run_id="our-run"):
        return runner.Lease(run_id, "test", self.file, self.directory)

    def test_both_locks_held_and_persistent_file_inode_survives_release(self):
        lease = self.lease().acquire()
        inode = self.file.stat().st_ino
        self.assertEqual(json.loads((self.directory / "owner.json").read_text())["run_id"], "our-run")
        with self.file.open("r+") as competitor:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(competitor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        lease.release()
        self.assertFalse(self.directory.exists())
        self.assertEqual(self.file.stat().st_ino, inode)
        with self.file.open("r+") as competitor:
            fcntl.flock(competitor, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_existing_legacy_directory_is_never_reclaimed(self):
        self.directory.mkdir()
        marker = self.directory / "foreign"
        marker.write_text("old does not mean abandoned")
        with self.assertRaises(runner.CoordinationError):
            self.lease().acquire()
        self.assertEqual(marker.read_text(), "old does not mean abandoned")
        self.assertFalse(self.file.exists())

    def test_legacy_file_contention_releases_only_our_directory(self):
        with self.file.open("w+") as competitor:
            competitor.write("persistent")
            competitor.flush()
            fcntl.flock(competitor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(runner.CoordinationError):
                self.lease().acquire()
            self.assertFalse(self.directory.exists())
            self.assertEqual(self.file.read_text(), "persistent")

    def test_second_wrapper_cannot_acquire_and_first_owner_survives(self):
        lease = self.lease().acquire()
        try:
            with self.assertRaises(runner.CoordinationError):
                self.lease("second").acquire()
            self.assertEqual(json.loads((self.directory / "owner.json").read_text())["run_id"], "our-run")
        finally:
            lease.release()

    def test_changed_owner_is_not_deleted(self):
        lease = self.lease().acquire()
        marker = self.directory / "owner.json"
        marker.write_text('{"run_id":"someone-else"}')
        with self.assertRaises(runner.CoordinationError):
            lease.release()
        self.assertTrue(marker.exists())
        self.assertIsNone(lease.fd)

    def test_symlink_file_is_not_followed_and_partial_directory_is_removed(self):
        target = self.root / "target"
        target.write_text("keep")
        self.file.symlink_to(target)
        with self.assertRaises(OSError):
            self.lease().acquire()
        self.assertEqual(target.read_text(), "keep")
        self.assertFalse(self.directory.exists())

    def test_malformed_owner_shape_is_not_deleted(self):
        lease = self.lease().acquire()
        marker = self.directory / "owner.json"
        marker.write_text("[]")
        with self.assertRaises(runner.CoordinationError):
            lease.release()
        self.assertEqual(marker.read_text(), "[]")
        self.assertIsNone(lease.fd)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.receipt_path = self.root / "receipt.json"
        self.lock_file, self.lock_directory = self.root / "lock", self.root / "lockdir"
        self.source = {"commit": "a" * 40, "sha256": "b" * 64}

    def arguments(self, *extra):
        return runner.parse_arguments([
            "--owner", "coordinator", "--task", "unit test", "--kind", "validation",
            "--receipt", str(self.receipt_path), "--cwd", str(self.root), *extra,
            "--", "not-a-real-command", "argument with spaces"])

    def lease(self, run_id, owner):
        return runner.Lease(run_id, owner, self.lock_file, self.lock_directory)

    def run_mocked(self, arguments=None, *, exit_code=0, sources=None, spawn_error=None):
        child = SimpleNamespace(pid=123456789, wait=Mock(return_value=exit_code))
        with patch.object(runner, "snapshot_source", side_effect=sources or [self.source, self.source]), \
                patch.object(runner.subprocess, "Popen", return_value=child, side_effect=spawn_error) as popen, \
                patch.object(runner, "group_exists", return_value=False), \
                contextlib.redirect_stderr(io.StringIO()):
            result = runner.execute(arguments or self.arguments(), lease_factory=self.lease)
        return result, popen, json.loads(self.receipt_path.read_text())

    def test_success_binds_source_artifact_and_only_two_environment_overrides(self):
        artifact = self.root / "report.json"
        artifact.write_bytes(b'{"report":"bytes"}\n')
        inherited = {"KEEP": "value", "PAGURA_RUN_ID": "old", "PAGURA_RUN_RECEIPT": "old-path"}
        with patch.dict(runner.os.environ, inherited, clear=True):
            result, popen, receipt = self.run_mocked(self.arguments("--artifact", str(artifact)))
        self.assertEqual(result, 0)
        self.assertEqual(receipt["status"], "completed")
        self.assertEqual(receipt["exit_code"], 0)
        self.assertEqual(receipt["wrapper_exit_code"], 0)
        self.assertTrue(receipt["source_unchanged"])
        self.assertEqual(receipt["source_before"], receipt["source_after"])
        self.assertEqual(receipt["artifacts"], {str(artifact): hashlib.sha256(artifact.read_bytes()).hexdigest()})
        self.assertEqual(set(receipt["environment_overrides"]), {"PAGURA_RUN_ID", "PAGURA_RUN_RECEIPT"})
        self.assertEqual(popen.call_args.kwargs["env"], {**inherited, **receipt["environment_overrides"]})
        self.assertEqual(popen.call_args.args[0], ["not-a-real-command", "argument with spaces"])
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        self.assertEqual(len(popen.call_args.kwargs["pass_fds"]), 1)
        self.assertNotIn("controlled", receipt["runner"])
        self.assertIsNotNone(receipt["finalized_at"])
        self.assertFalse(self.lock_directory.exists())

    def test_child_failure_and_signal_exit_are_preserved(self):
        for child_code, wrapper_code in ((7, 7), (-signal.SIGTERM, 128 + signal.SIGTERM)):
            with self.subTest(child_code=child_code):
                self.receipt_path.unlink(missing_ok=True)
                result, _, receipt = self.run_mocked(exit_code=child_code)
                self.assertEqual(result, wrapper_code)
                self.assertEqual(receipt["exit_code"], child_code)
                self.assertEqual(receipt["status"], "failed")

    def test_spawn_failure_has_terminal_receipt_and_releases_locks(self):
        result, _, receipt = self.run_mocked(spawn_error=FileNotFoundError("missing executable"))
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "spawn-failed")
        self.assertIsNone(receipt["exit_code"])
        self.assertFalse(self.lock_directory.exists())

    def test_source_drift_and_missing_artifacts_fail_evidence(self):
        changed = {**self.source, "sha256": "c" * 64}
        missing = self.root / "missing.json"
        result, _, receipt = self.run_mocked(self.arguments("--artifact", str(missing)), sources=[self.source, changed])
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "evidence-failed")
        self.assertEqual(receipt["exit_code"], 0)
        self.assertFalse(receipt["source_unchanged"])
        self.assertIn(str(missing), receipt["artifact_errors"])
        self.assertFalse(receipt["artifacts"])

    def test_nonregular_artifacts_are_rejected_without_waiting_for_fifo_input(self):
        fifo = self.root / "not-a-report"
        os.mkfifo(fifo)
        with self.assertRaises(OSError):
            runner.file_digest(fifo)

    def test_initial_source_failure_runs_no_child(self):
        result, popen, receipt = self.run_mocked(sources=[OSError("source unreadable"), self.source])
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        popen.assert_not_called()
        self.assertEqual(receipt["status"], "evidence-failed")
        self.assertIsNone(receipt["started_at"])

    def test_existing_receipt_is_not_overwritten_and_no_child_starts(self):
        self.receipt_path.write_text("existing evidence")
        with patch.object(runner.subprocess, "Popen") as popen, \
                patch.object(runner, "snapshot_source") as snapshot, \
                contextlib.redirect_stderr(io.StringIO()):
            result = runner.execute(self.arguments(), lease_factory=self.lease)
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        popen.assert_not_called()
        snapshot.assert_not_called()
        self.assertEqual(self.receipt_path.read_text(), "existing evidence")
        self.assertFalse(self.lock_directory.exists())

    def test_contention_runs_no_subprocess_and_creates_no_receipt(self):
        self.lock_directory.mkdir()
        with patch.object(runner.subprocess, "Popen") as popen, \
                patch.object(runner, "snapshot_source") as snapshot, \
                contextlib.redirect_stderr(io.StringIO()):
            result = runner.execute(self.arguments(), lease_factory=self.lease)
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        popen.assert_not_called()
        snapshot.assert_not_called()
        self.assertFalse(self.receipt_path.exists())
        self.assertTrue(self.lock_directory.exists())

    def test_running_receipt_failure_still_waits_for_child_and_fails_evidence(self):
        write = runner.write_receipt
        count = 0

        def fail_running(path, receipt, **kwargs):
            nonlocal count
            count += 1
            if count == 3:
                raise OSError("disk temporarily unavailable")
            return write(path, receipt, **kwargs)

        with patch.object(runner, "write_receipt", side_effect=fail_running):
            result, popen, receipt = self.run_mocked()
        popen.return_value.wait.assert_called_once()
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "evidence-failed")
        self.assertEqual(receipt["exit_code"], 0)

    def test_final_receipt_failure_leaves_running_evidence_and_returns_failure(self):
        write = runner.write_receipt

        def fail_final(path, receipt, **kwargs):
            if receipt["finalized_at"] is not None:
                raise OSError("final write failed")
            return write(path, receipt, **kwargs)

        with patch.object(runner, "write_receipt", side_effect=fail_final):
            result, _, receipt = self.run_mocked()
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "running")
        self.assertIsNone(receipt["finalized_at"])
        self.assertFalse(self.lock_directory.exists())

    def test_cleanup_failure_is_not_reported_as_wrapper_success(self):
        release = runner.Lease.release

        def cleanup_then_fail(lease):
            release(lease)
            raise OSError("cleanup could not be certified")

        with patch.object(runner.Lease, "release", cleanup_then_fail):
            result, _, receipt = self.run_mocked()
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "evidence-failed")
        self.assertEqual(receipt["wrapper_exit_code"], runner.INFRASTRUCTURE_FAILURE)

    def test_cleanup_and_terminal_write_failure_cannot_leave_a_completed_receipt(self):
        release, write = runner.Lease.release, runner.write_receipt
        failed_terminal_write = False

        def cleanup_then_fail(lease):
            release(lease)
            raise OSError("cleanup could not be certified")

        def fail_failure_receipt(path, receipt, **kwargs):
            nonlocal failed_terminal_write
            if receipt["status"] == "evidence-failed":
                failed_terminal_write = True
                raise OSError("terminal receipt storage unavailable")
            return write(path, receipt, **kwargs)

        with patch.object(runner.Lease, "release", cleanup_then_fail), \
                patch.object(runner, "write_receipt", side_effect=fail_failure_receipt):
            result, _, receipt = self.run_mocked()
        self.assertTrue(failed_terminal_write)
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "running")
        self.assertIsNone(receipt["wrapper_exit_code"])
        self.assertIsNone(receipt["finalized_at"])

    def test_success_is_persisted_only_after_lock_cleanup(self):
        release = runner.Lease.release

        def verify_running_then_release(lease):
            receipt = json.loads(self.receipt_path.read_text())
            self.assertEqual(receipt["status"], "running")
            self.assertIsNone(receipt["finalized_at"])
            release(lease)

        with patch.object(runner.Lease, "release", verify_running_then_release):
            result, _, receipt = self.run_mocked()
        self.assertEqual(result, 0)
        self.assertEqual(receipt["status"], "completed")
        self.assertFalse(self.lock_directory.exists())

    def test_child_mutating_owner_json_to_array_fails_receipt_and_preserves_directory(self):
        child = SimpleNamespace(pid=123456789, wait=Mock(return_value=0))

        def spawn(*_args, **_kwargs):
            (self.lock_directory / "owner.json").write_text("[]")
            return child

        with patch.object(runner, "snapshot_source", return_value=self.source), \
                patch.object(runner.subprocess, "Popen", side_effect=spawn), \
                patch.object(runner, "group_exists", return_value=False), \
                contextlib.redirect_stderr(io.StringIO()):
            result = runner.execute(self.arguments(), lease_factory=self.lease)
        receipt = json.loads(self.receipt_path.read_text())
        self.assertEqual(result, runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual(receipt["status"], "evidence-failed")
        self.assertEqual(receipt["wrapper_exit_code"], runner.INFRASTRUCTURE_FAILURE)
        self.assertEqual((self.lock_directory / "owner.json").read_text(), "[]")

    def test_receipt_update_rejects_invalid_json_and_nonobject_shapes(self):
        for content in ("[]", "null", "42", '"text"', "{"):
            with self.subTest(content=content):
                self.receipt_path.write_text(content)
                with self.assertRaises(runner.CoordinationError):
                    runner.write_receipt(self.receipt_path, {"run_id": "our-run"})
                self.assertEqual(self.receipt_path.read_text(), content)

    def test_dry_run_has_no_subprocess_lock_or_write(self):
        output = io.StringIO()
        with patch.object(runner, "execute") as execute, patch.object(runner.subprocess, "run") as run, \
                patch.object(runner.subprocess, "Popen") as popen, \
                patch.object(runner, "write_receipt") as write, contextlib.redirect_stdout(output):
            result = runner.main(["--owner", "test", "--task", "plan", "--kind", "validation",
                                  "--receipt", str(self.receipt_path), "--dry-run", "--", "arbitrary-command"])
        self.assertEqual(result, 0)
        execute.assert_not_called()
        run.assert_not_called()
        popen.assert_not_called()
        write.assert_not_called()
        self.assertIn("no subprocesses, lock acquisition or file writes", output.getvalue())

    def test_benchmark_requires_artifacts_and_receipt_cannot_hash_itself(self):
        cases = [["--kind", "benchmark"], ["--artifact", str(self.receipt_path)],
                 ["--artifact", "same", "--artifact", "same"]]
        for options in cases:
            with self.subTest(options=options), contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit) as error:
                self.arguments(*options)
            self.assertEqual(error.exception.code, 2)


class CancellationTests(unittest.TestCase):
    def test_signal_is_forwarded_only_to_owned_child_group_including_spawn_race(self):
        cancellation = runner.Cancellation()
        with patch.object(runner.os, "killpg") as kill:
            cancellation.receive(signal.SIGTERM, None)
            kill.assert_not_called()
            cancellation.attach(1234)
            kill.assert_called_once_with(1234, signal.SIGTERM)
            cancellation.receive(signal.SIGINT, None)
            self.assertEqual(kill.call_args.args, (1234, signal.SIGINT))
        self.assertEqual(cancellation.signum, signal.SIGTERM)

    def test_uncertain_forwarding_does_not_escape_and_abandon_child(self):
        cancellation = runner.Cancellation()
        cancellation.attach(1234)
        with patch.object(runner.os, "killpg", side_effect=PermissionError("cannot signal")):
            cancellation.receive(signal.SIGTERM, None)
        self.assertIn("cancellation forwarding failed", cancellation.errors[0])

    def test_group_uncertainty_is_treated_as_live(self):
        with patch.object(runner.os, "killpg", side_effect=PermissionError("unknown")):
            self.assertTrue(runner.group_exists(1234))

    def test_wait_does_not_return_while_descendants_remain(self):
        child = SimpleNamespace(pid=1234, wait=Mock(return_value=4))
        with patch.object(runner, "group_exists", side_effect=[True, True, False]) as probe, \
                patch.object(runner.time, "sleep") as sleep, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(runner.wait_for_group(child), 4)
        child.wait.assert_called_once()
        self.assertEqual(probe.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_cancellation_cannot_be_reported_as_completed_even_if_child_exits_zero(self):
        source = {"commit": "a" * 40, "sha256": "b" * 64}
        receipt = {"source_before": source, "source_after": None, "source_unchanged": False,
                   "exit_code": 0, "status": "running", "errors": [], "artifacts": {}, "artifact_errors": {}}
        cancellation = runner.Cancellation()
        cancellation.signum = signal.SIGINT
        with patch.object(runner, "snapshot_source", return_value=source):
            result = runner.finish_receipt(receipt, SimpleNamespace(artifact=[]), cancellation)
        self.assertEqual(result, 128 + signal.SIGINT)
        self.assertEqual(receipt["status"], "canceled")


class BenchmarkCancellationIntegrationTests(unittest.TestCase):
    def test_wrapper_retains_lease_until_detached_renderer_and_worker_exit(self):
        """Real Python-only processes exercise the standard measurement boundary."""
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            tools = str(Path(runner.__file__).resolve().parent)
            receipt_path = directory / "runner.json"
            worker = directory / "worker.py"
            worker.write_text(textwrap.dedent("""
                import os, signal, time
                from pathlib import Path
                root = Path(__file__).parent
                stopping = False
                def cancel(signum, frame):
                    global stopping
                    stopping = True
                signal.signal(signal.SIGTERM, cancel)
                signal.signal(signal.SIGINT, cancel)
                (root / 'worker-ready').write_text(str(os.getpid()))
                while not stopping:
                    time.sleep(0.02)
                time.sleep(0.15)
                (root / 'worker-stopped').write_text('stopped')
                """))
            renderer = directory / "renderer.py"
            renderer.write_text(textwrap.dedent("""
                import os, signal, subprocess, sys, time
                from pathlib import Path
                root = Path(__file__).parent
                stopping = False
                def cancel(signum, frame):
                    global stopping
                    stopping = True
                signal.signal(signal.SIGTERM, cancel)
                signal.signal(signal.SIGINT, cancel)
                child = subprocess.Popen([sys.executable, str(root / 'worker.py')])
                while not (root / 'worker-ready').exists():
                    time.sleep(0.02)
                (root / 'renderer-ready').write_text(str(os.getpid()))
                while not stopping:
                    time.sleep(0.02)
                child.wait(timeout=3)
                (root / 'renderer-stopped').write_text('stopped')
                """))
            bridge = directory / "bridge.py"
            bridge.write_text(textwrap.dedent(f"""
                import signal, sys
                from pathlib import Path
                sys.path.insert(0, {tools!r})
                from benchkit.measurement import run_process
                root = Path(__file__).parent
                previous = {{sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}}
                try:
                    run_process([sys.executable, str(root / 'renderer.py')],
                                {{'timeout_seconds': 5, 'measure_rss': False}})
                except BaseException:
                    assert all(signal.getsignal(sig) == handler for sig, handler in previous.items())
                    (root / 'bridge-stopped').write_text('handlers restored')
                    raise
                (root / 'later-sample').write_text('must never execute')
                """))
            harness = textwrap.dedent(f"""
                import sys
                from pathlib import Path
                sys.path.insert(0, {tools!r})
                import runner
                root = Path({str(directory)!r})
                runner.snapshot_source = lambda: {{'commit': 'a' * 40, 'sha256': 'b' * 64}}
                args = runner.parse_arguments([
                    '--owner', 'unit-test', '--task', 'cancellation-fixture', '--kind', 'validation',
                    '--cwd', str(root), '--receipt', str(root / 'runner.json'),
                    '--', sys.executable, str(root / 'bridge.py')])
                factory = lambda run_id, owner: runner.Lease(run_id, owner, root / 'lock', root / 'lockdir')
                raise SystemExit(runner.execute(args, lease_factory=factory))
                """)
            outer = subprocess.Popen([sys.executable, "-c", harness], start_new_session=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            finished = False
            try:
                deadline = time.monotonic() + 8
                ready = directory / "renderer-ready"
                while not ready.exists() and time.monotonic() < deadline and outer.poll() is None:
                    time.sleep(0.02)
                self.assertTrue(ready.exists(), "fixture renderer did not become ready")
                outer.send_signal(signal.SIGTERM)
                stdout, stderr = outer.communicate(timeout=10)
                self.assertEqual(outer.returncode, 128 + signal.SIGTERM, stdout + stderr)
                receipt = json.loads(receipt_path.read_text())
                self.assertEqual(receipt["status"], "canceled")
                self.assertEqual(receipt["cancellation_signal"], signal.SIGTERM)
                for name in ("worker-stopped", "renderer-stopped", "bridge-stopped"):
                    self.assertTrue((directory / name).exists(), f"{name} absent: {stderr}")
                self.assertFalse((directory / "later-sample").exists())
                for name in ("worker-ready", "renderer-ready"):
                    with self.assertRaises(ProcessLookupError):
                        os.kill(int((directory / name).read_text()), 0)
                lease = runner.Lease("after-cancel", "unit-test", directory / "lock", directory / "lockdir")
                lease.acquire()
                lease.release()
                finished = True
            finally:
                if not finished:
                    # Cleanup only process groups created by this controlled fixture.
                    groups = {outer.pid}
                    if receipt_path.exists():
                        child_pid = json.loads(receipt_path.read_text()).get("child_pid")
                        if child_pid:
                            groups.add(child_pid)
                    if (directory / "renderer-ready").exists():
                        groups.add(int((directory / "renderer-ready").read_text()))
                    for pid in groups:
                        try:
                            os.killpg(pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    outer.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
