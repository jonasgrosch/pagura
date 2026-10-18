"""Small isolated supervisor: report engine wall time and wait4 CPU/RSS/status.

Run with Python -I -S so importing the harness cannot inflate the address space
from which the engine is spawned. The supervisor's own inherited maxrss is never
reported. The engine inherits the supervisor's process group for timeout kills.
"""
import os
import signal
import sys
import time


def main():
    accounting = int(sys.argv[1])
    os.set_inheritable(accounting, False)
    command = sys.argv[2:]
    started = time.perf_counter_ns()
    try:
        pid = os.posix_spawnp(command[0], command, os.environ,
                              setsigdef=(signal.SIGPIPE, signal.SIGXFSZ))
    except OSError as error:
        os.write(accounting, f"error {error}\n".encode())
        return
    _, status, usage = os.wait4(pid, 0)
    elapsed = time.perf_counter_ns() - started
    user, system = round(usage.ru_utime * 1e9), round(usage.ru_stime * 1e9)
    peak = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    # Fixed fields: marker, exit code, wall ns, user ns, system ns, RSS bytes.
    os.write(accounting, f"ok {os.waitstatus_to_exitcode(status)} {elapsed} {user} {system} {peak}\n".encode())


if __name__ == "__main__":
    main()
