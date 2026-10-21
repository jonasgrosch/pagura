"""Run one foreground job under Pagura's cooperative local runner locks."""
import argparse
import errno
import fcntl
import hashlib
import json
import os
import platform
import shlex
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from benchkit.config import ROOT, source_digest
from benchkit.process_group import ProcessGroup, ProcessGroupError, require_group_support

LOCK_FILE = Path("/tmp/pagura-bench.lock")
LOCK_DIRECTORY = Path("/tmp/pagura-bench.lockdir")
INFRASTRUCTURE_FAILURE = 125


def timestamp():
    return datetime.now(timezone.utc).isoformat()


class CoordinationError(RuntimeError):
    """No job may start when the shared coordination protocol is unavailable."""


def require_owner(path, run_id, label):
    try:
        value = json.loads(path.read_text())
    except ValueError as error:
        raise CoordinationError(f"{label} is invalid JSON; refusing cleanup or overwrite") from error
    if not isinstance(value, dict) or value.get("run_id") != run_id:
        raise CoordinationError(f"{label} ownership changed; refusing cleanup or overwrite")


class Lease:
    """Hold both legacy mechanisms; never reclaim stale or foreign ownership."""

    def __init__(self, run_id, owner, lock_file=LOCK_FILE, lock_directory=LOCK_DIRECTORY):
        self.run_id, self.owner = run_id, owner
        self.lock_file, self.directory = Path(lock_file), Path(lock_directory)
        self.fd = None
        self.directory_identity = None
        self.acquired_at = None

    def acquire(self):
        try:
            self.directory.mkdir(mode=0o700)
        except FileExistsError as error:
            raise CoordinationError(f"runner lock directory exists: {self.directory}; no takeover attempted") from error
        try:
            info = self.directory.stat()
            self.directory_identity = (info.st_dev, info.st_ino)
            with (self.directory / "owner.json").open("x") as stream:
                json.dump({"run_id": self.run_id, "owner": self.owner,
                           "pid": os.getpid(), "hostname": socket.gethostname()}, stream)
            self.fd = os.open(self.lock_file, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
            if not stat.S_ISREG(os.fstat(self.fd).st_mode):
                raise CoordinationError("runner file lock is not a regular file")
            try:
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EACCES):
                    raise CoordinationError(f"runner file lock is held: {self.lock_file}") from error
                raise
            self.acquired_at = timestamp()
        except BaseException:
            self.release()
            raise
        return self

    def release(self):
        try:
            if self.directory_identity is not None:
                info = self.directory.lstat()
                if (info.st_dev, info.st_ino) != self.directory_identity:
                    raise CoordinationError("lock directory changed; refusing cleanup")
                owner_path = self.directory / "owner.json"
                if owner_path.exists():
                    require_owner(owner_path, self.run_id, "lock")
                    owner_path.unlink()
                self.directory.rmdir()
                self.directory_identity = None
        finally:
            if self.fd is not None:
                # Keep the persistent file: unlinking would permit two locked inodes.
                os.close(self.fd)
                self.fd = None


class Cancellation:
    """Forward only received cancellation signals to our own child process group."""

    def __init__(self):
        self.signum = None
        self.pid = None
        self.group = None
        self.previous = {}
        self.errors = []

    def receive(self, signum, _frame):
        self.signum = self.signum or signum
        self.forward(signum)

    def forward(self, signum):
        if self.group is not None:
            self.group.forward(signum)

    def attach(self, child):
        self.pid = child.pid
        self.group = ProcessGroup(child)
        self.errors = self.group.errors
        if self.signum is not None:
            self.forward(self.signum)

    def __enter__(self):
        for signum in (signal.SIGINT, signal.SIGTERM):
            self.previous[signum] = signal.signal(signum, self.receive)
        return self

    def __exit__(self, *_args):
        for signum, previous in self.previous.items():
            signal.signal(signum, previous)


def wait_for_group(child, group=None):
    group = group or ProcessGroup(child)
    try:
        group.wait_leader()
        group.drain(interval=0.1)
        group.check()
    finally:
        # Forwarding is disabled before reap, including exception paths. A
        # failed snapshot aborts the job; it is never interpreted as absence.
        if group.leader_exited:
            result = group.reap()
        else:
            group.disarm()
    return result


def snapshot_source():
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                            capture_output=True, text=True, check=True)
    return {"commit": commit.stdout.strip(), "sha256": source_digest()}


def file_digest(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise OSError("artifact is not a regular file")
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()
    finally:
        os.close(descriptor)


def write_receipt(path, receipt, *, create=False):
    text = json.dumps(receipt, indent=2, sort_keys=True, allow_nan=False) + "\n"
    if create:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("x") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        return
    require_owner(path, receipt["run_id"], "receipt")
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".runner-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def initial_receipt(arguments, run_id, lease):
    return {"schema": 1, "run_id": run_id, "owner": arguments.owner,
            "task": arguments.task, "kind": arguments.kind, "argv": arguments.command,
            "cwd": str(arguments.cwd), "source_root": str(ROOT),
            "runner": {"hostname": socket.gethostname(), "system": platform.system(),
                       "machine": platform.machine(), "pid": os.getpid()},
            "coordination": {"scope": "cooperative", "lock_file": str(lease.lock_file),
                             "lock_directory": str(lease.directory), "acquired_at": lease.acquired_at},
            "environment_overrides": {"PAGURA_RUN_ID": run_id, "PAGURA_RUN_RECEIPT": str(arguments.receipt)},
            "source_before": None, "source_after": None, "source_unchanged": False,
            "started_at": None, "ended_at": None, "duration_ns": None, "child_pid": None,
            "exit_code": None, "wrapper_exit_code": None, "cancellation_signal": None,
            "status": "running", "artifacts": {}, "artifact_errors": {}, "errors": [],
            "finalized_at": None}


def finish_receipt(receipt, arguments, cancellation):
    try:
        receipt["source_after"] = snapshot_source()
        receipt["source_unchanged"] = receipt["source_before"] == receipt["source_after"]
        if not receipt["source_unchanged"]:
            receipt["errors"].append("source identity changed during the job")
    except (OSError, subprocess.SubprocessError) as error:
        receipt["errors"].append(f"final source identity unavailable: {error}")
    for path in arguments.artifact:
        try:
            receipt["artifacts"][str(path)] = file_digest(path)
        except OSError as error:
            receipt["artifact_errors"][str(path)] = str(error)
    receipt["cancellation_signal"] = cancellation.signum
    receipt["errors"].extend(cancellation.errors)
    code = receipt["exit_code"]
    if cancellation.signum is not None:
        receipt["status"], result = "canceled", 128 + cancellation.signum
    elif receipt["status"] == "spawn-failed":
        result = INFRASTRUCTURE_FAILURE
    elif code is not None and code != 0:
        receipt["status"], result = "failed", code if code > 0 else 128 - code
    elif code is None or receipt["errors"] or receipt["artifact_errors"]:
        receipt["status"], result = "evidence-failed", INFRASTRUCTURE_FAILURE
    else:
        receipt["status"], result = "completed", 0
    receipt["wrapper_exit_code"] = result
    receipt["finalized_at"] = timestamp()
    return result


def execute(arguments, *, lease_factory=Lease):
    run_id = str(uuid.uuid4())
    lease = lease_factory(run_id, arguments.owner)
    receipt = None
    created = False
    result = INFRASTRUCTURE_FAILURE
    try:
        require_group_support()
        lease.acquire()
        with Cancellation() as cancellation:
            receipt = initial_receipt(arguments, run_id, lease)
            write_receipt(arguments.receipt, receipt, create=True)
            created = True
            try:
                receipt["source_before"] = snapshot_source()
                write_receipt(arguments.receipt, receipt)
            except (OSError, subprocess.SubprocessError) as error:
                receipt["errors"].append(f"initial source identity unavailable: {error}")
            if not receipt["errors"] and cancellation.signum is None:
                receipt["started_at"] = timestamp()
                start = time.monotonic_ns()
                try:
                    child = subprocess.Popen(arguments.command, cwd=arguments.cwd,
                                             env={**os.environ, **receipt["environment_overrides"]},
                                             start_new_session=True, pass_fds=(lease.fd,))
                except OSError as error:
                    receipt["status"] = "spawn-failed"
                    receipt["errors"].append(str(error))
                else:
                    receipt["child_pid"] = child.pid
                    try:
                        cancellation.attach(child)
                        # A failed running-checkpoint update must not orphan the child.
                        try:
                            write_receipt(arguments.receipt, receipt)
                        except (OSError, ValueError, CoordinationError) as error:
                            receipt["errors"].append(f"running receipt update failed: {error}")
                    finally:
                        receipt["exit_code"] = wait_for_group(child, cancellation.group)
                        cancellation.pid = None
                receipt["ended_at"] = timestamp()
                receipt["duration_ns"] = time.monotonic_ns() - start
            result = finish_receipt(receipt, arguments, cancellation)
    except (OSError, ValueError, subprocess.SubprocessError, CoordinationError, ProcessGroupError) as error:
        print(f"runner: {error}", file=sys.stderr)
        if created:
            receipt["errors"].append(f"runner execution failed: {error}")
            receipt["status"] = "evidence-failed"
            receipt["wrapper_exit_code"] = INFRASTRUCTURE_FAILURE
        result = INFRASTRUCTURE_FAILURE
    finally:
        try:
            lease.release()
        except (OSError, ValueError, CoordinationError) as error:
            print(f"runner: lock cleanup failed; manual review required: {error}", file=sys.stderr)
            result = INFRASTRUCTURE_FAILURE
            if created:
                receipt["errors"].append(f"lock cleanup failed: {error}")
                receipt["status"] = "evidence-failed"
                receipt["wrapper_exit_code"] = result
    if created:
        # Keep the on-disk receipt running until cleanup has a known outcome.
        # A failed terminal write must never leave an earlier success behind.
        receipt["finalized_at"] = timestamp()
        try:
            write_receipt(arguments.receipt, receipt)
        except (OSError, ValueError, CoordinationError) as error:
            print(f"runner: final receipt write failed: {error}", file=sys.stderr)
            print(f"runner: receipt remains incomplete: {arguments.receipt}", file=sys.stderr)
            result = INFRASTRUCTURE_FAILURE
    return result


def parse_arguments(argv):
    parser = argparse.ArgumentParser(description=__doc__, epilog=(
        "Cooperative locking is not a controlled-runner attestation. Do not nest inside fleet bench. "
        "Only foreground commands are supported; no stale-lock takeover or automatic kill is performed."))
    parser.add_argument("--owner", required=True)
    parser.add_argument("--task", required=True)
    parser.add_argument("--kind", required=True, choices=("validation", "benchmark", "profile", "fuzz", "other"))
    parser.add_argument("--receipt", required=True, type=Path, help="new JSON lifecycle receipt; existing files are refused")
    parser.add_argument("--cwd", type=Path, default=ROOT)
    parser.add_argument("--artifact", type=Path, action="append", default=[],
                        help="explicit final output file to hash; repeatable and required for benchmark jobs")
    parser.add_argument("--dry-run", action="store_true", help="print the plan without subprocesses, lock acquisition or file writes")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="foreground command argv after --")
    arguments = parser.parse_args(argv)
    if arguments.command[:1] == ["--"]:
        arguments.command = arguments.command[1:]
    if not arguments.command:
        parser.error("a foreground command after -- is required")
    if not arguments.owner.strip() or not arguments.task.strip():
        parser.error("owner and task must be nonempty")
    if arguments.kind == "benchmark" and not arguments.artifact:
        parser.error("benchmark jobs require explicit --artifact output files")
    arguments.cwd = arguments.cwd.resolve()
    arguments.receipt = (arguments.cwd / arguments.receipt).resolve()
    arguments.artifact = [(arguments.cwd / path).resolve() for path in arguments.artifact]
    if arguments.receipt in arguments.artifact:
        parser.error("the receipt cannot hash itself as an artifact")
    if len(arguments.artifact) != len(set(arguments.artifact)):
        parser.error("artifact paths must be unique")
    return arguments


def main(argv=None):
    arguments = parse_arguments(argv)
    if arguments.dry_run:
        print("Dry run: no subprocesses, lock acquisition or file writes.")
        print(f"Working directory: {arguments.cwd}")
        print(f"Source identity: checkout containing this wrapper ({ROOT})")
        print(f"Cooperative locks: {LOCK_DIRECTORY} and {LOCK_FILE}")
        print(f"Receipt: {arguments.receipt}")
        print("Environment: inherited plus PAGURA_RUN_ID=<new UUID> and "
              f"PAGURA_RUN_RECEIPT={shlex.quote(str(arguments.receipt))}")
        print("Planned: " + shlex.join(arguments.command))
        for path in arguments.artifact:
            print(f"Final artifact to hash: {path}")
        return 0
    return execute(arguments)


if __name__ == "__main__":
    sys.exit(main())
