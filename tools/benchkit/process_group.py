"""Keep an owned POSIX session leader unreaped until its group is drained.

The unreaped child reserves its PID, so its process-group number cannot be
reused while forwarding is armed. This covers the original process group;
descendants that deliberately leave it are outside this foreground contract.
The process-table snapshot is a cooperative check, not an atomic containment
proof against a descendant deliberately racing fork and exit.
"""
import os
import subprocess
import threading
import time


class ProcessGroupError(RuntimeError):
    """Group lifetime or cleanup could not be verified; no sample may pass."""


def require_group_support():
    if not all(hasattr(os, name) for name in ("waitid", "P_PID", "WEXITED", "WNOWAIT")):
        raise ProcessGroupError("owned process groups require waitid(WNOWAIT)")


class ProcessGroup:
    """Own a freshly spawned start_new_session child; no other code may reap it."""

    def __init__(self, child):
        require_group_support()
        self.child = child
        self.errors = []
        self.leader_exited = False
        self._armed = True
        # A Python signal can interrupt a main-thread transition holding this
        # lock. Reentrancy prevents the scoped signal handler self-deadlocking.
        self._lock = threading.RLock()

    def forward(self, signum):
        with self._lock:
            if not self._armed:
                return
            try:
                os.killpg(self.child.pid, signum)
            except ProcessLookupError:
                pass
            except OSError as error:
                # Signal handlers and watchdogs must not abandon cleanup.
                self.errors.append(f"owned process-group forwarding failed: {error}")

    def wait_leader(self):
        if not self.leader_exited:
            try:
                result = os.waitid(os.P_PID, self.child.pid, os.WEXITED | os.WNOWAIT)
            except ChildProcessError as error:
                self.disarm()
                raise ProcessGroupError("owned leader was reaped outside its lifecycle") from error
            if result is None or result.si_pid != self.child.pid:
                self.disarm()
                raise ProcessGroupError("owned leader exit could not be identified")
            self.leader_exited = True

    def live_members(self):
        if not self.leader_exited or not self._armed:
            raise ProcessGroupError("group inspection requires an unreaped exited leader")
        try:
            result = subprocess.run(["ps", "-A", "-o", "pid=,pgid=,stat="],
                                    capture_output=True, text=True, check=True, timeout=5,
                                    env={**os.environ, "LC_ALL": "C"})
            anchor, live = False, False
            for line in result.stdout.splitlines():
                fields = line.split()
                if len(fields) != 3:
                    raise ValueError("incomplete process-group snapshot")
                pid, group = int(fields[0]), int(fields[1])
                if pid == self.child.pid:
                    anchor = group == self.child.pid and fields[2].startswith("Z")
                elif group == self.child.pid and not fields[2].startswith("Z"):
                    live = True
            if not anchor:
                # Do not signal a number whose anchor is no longer observable.
                self.disarm()
                raise ValueError("unreaped process-group leader missing from snapshot")
            return live
        except (OSError, ValueError, UnicodeError, subprocess.SubprocessError) as error:
            raise ProcessGroupError(f"owned process-group cleanup is unverified: {error}") from error

    def drain(self, interval=0.01):
        remaining = self.live_members()
        drained = remaining
        while remaining:
            time.sleep(interval)
            remaining = self.live_members()
        return drained

    def disarm(self):
        with self._lock:
            self._armed = False

    def reap(self):
        # The caller must first cancel/join every watchdog, outside this lock.
        # Disarm before wait: even a signal interrupting wait cannot target a
        # numeric group ID after the kernel releases the child's PID.
        self.disarm()
        if not self.leader_exited:
            raise ProcessGroupError("refusing to reap before an identified leader exit")
        return self.child.wait()

    def check(self):
        if self.errors:
            raise ProcessGroupError("; ".join(self.errors))
