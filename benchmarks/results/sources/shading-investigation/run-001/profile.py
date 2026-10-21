#!/usr/bin/env python3
"""One retained-product sparse64 Exact profile; no comparative speed/admission claim."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import platform
import signal
import stat
import subprocess
import sys
import threading
import time

sys.dont_write_bytecode = True
MAX_FILE = 64 * 1024 * 1024


class Cancelled(BaseException):
    def __init__(self, signum):
        self.signum = signum


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_regular(path):
    """Safe even before the shared helper checkout has been authenticated."""
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= MAX_FILE,
                'Oversized/nonregular profile file: ' + str(path))
        raw = handle.read(MAX_FILE + 1)
        require(len(raw) <= MAX_FILE, 'Profile file grew beyond64MiB: ' + str(path))
        return raw


def fnv1a64(raw):
    value = 0xcbf29ce484222325
    for byte in raw:
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return f'{value:016x}'


def owned_profile(binary, input_path, sampler, directory, record, ProcessGroup, require_group_support):
    """Only these two new sessions are signal targets; never attach to existing work."""
    require_group_support()
    groups, cancelled, timed_out = [], None, False
    watchdog, watchdog_started = None, False
    previous = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}

    def forward(signum):
        for group in tuple(groups):
            group.forward(signum)

    def cancel(signum, _frame):
        nonlocal cancelled
        if cancelled is None:
            cancelled = signum
        forward(signum)

    def expire():
        nonlocal timed_out
        timed_out = True
        forward(signal.SIGKILL)

    def spawn(command, stdout, stderr, role):
        require(not timed_out and cancelled is None, 'Cancelled/expired before child launch')
        child = subprocess.Popen(command, cwd=record['root'], stdout=stdout, stderr=stderr,
                                 start_new_session=True)
        group = ProcessGroup(child)
        groups.append(group)
        record['children'].append({'role': role, 'pid': child.pid, 'argv': command})
        # A signal can arrive between Popen and attaching the owned group.
        if cancelled is not None:
            group.forward(cancelled)
        elif timed_out:
            group.forward(signal.SIGKILL)
        return group

    def alive(group):
        result = os.waitid(os.P_PID, group.child.pid, os.WEXITED | os.WNOWAIT | os.WNOHANG)
        if result is None:
            return True
        require(result.si_pid == group.child.pid, 'Unexpected owned-child status')
        group.leader_exited = True
        return False

    completed, failure = False, None
    try:
        for signum in previous:
            signal.signal(signum, cancel)
        watchdog = threading.Timer(120, expire)
        watchdog.daemon = True
        watchdog.start()
        watchdog_started = True
        with (directory / 'renders.txt').open('xb') as renders, (directory / 'renderer-stderr.txt').open('xb') as errors, \
             (directory / 'sample-command.txt').open('xb') as sample_log:
            renderer = spawn([str(binary), 'microbench', str(input_path), 'render-warm', '700', '72', '--accuracy', record['mode']], renders, errors, 'renderer')
            time.sleep(.1)
            require(alive(renderer), 'Renderer ended before the fixed profiler attachment')
            require(cancelled is None and not timed_out, 'Cancelled/expired before profiler attachment')
            command = [str(sampler), str(renderer.child.pid), '3', '1', '-mayDie', '-file', str(directory / 'sample.txt')]
            capture_start = time.monotonic_ns()
            profiler = spawn(command, sample_log, subprocess.STDOUT, 'sampler')
            profiler.wait_leader()
            record['sampler_wait_elapsed_ns'] = time.monotonic_ns() - capture_start
            record['renderer_alive_when_sampler_returned'] = alive(renderer)
            if record['renderer_alive_when_sampler_returned']:
                vm = spawn(['/usr/bin/vmmap', '-summary', str(renderer.child.pid)], sample_log, subprocess.STDOUT, 'vmmap')
                vm.wait_leader()
            renderer.wait_leader()
            completed = True
    except BaseException as error:
        failure = error
    finally:
        # Keep the common watchdog armed until both original process groups are
        # drained. It bounds a child that ignores graceful cancellation.
        cleanup_errors = []
        if not completed:
            forward(cancelled if cancelled is not None else signal.SIGTERM)
        if groups and not watchdog_started:
            expire()
        for group in groups:
            try:
                group.wait_leader()
                group.drain()
            except BaseException as error:
                cleanup_errors.append(f'{type(error).__name__}: {error}')
        try:
            if watchdog_started:
                watchdog.cancel()
                watchdog.join()
            for group, child_record in zip(groups, record['children']):
                try:
                    child_record['exit_code'] = group.reap()
                    group.check()
                except BaseException as error:
                    cleanup_errors.append(f'{type(error).__name__}: {error}')
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)
        record['cleanup_errors'] = cleanup_errors
        record['timed_out'] = timed_out
        record['cancellation_signal'] = cancelled
        require(not cleanup_errors, 'Owned process cleanup could not be verified')
    if cancelled is not None:
        raise Cancelled(cancelled)
    require(not timed_out, 'Fixed 120-second owned-child deadline exceeded')
    if failure is not None:
        raise failure
    require(len(record['children']) == 3 and all(c.get('exit_code') == 0 for c in record['children']),
            'Renderer or sampler failed; no retry')


sys.path.insert(0,str(Path(__file__).resolve().parent / 'unused'))
root=Path('/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura')
sys.path.insert(0,str(root/'tools'))
from benchkit.process_group import ProcessGroup,require_group_support
out=Path(__file__).resolve().parent
binary=Path('/private/tmp/current-stack-hayro-pagura-402e3afe/release/pagura')
prepared=json.loads((root/'benchmarks/results/local/current-stack-hayro/prepared.json').read_text())
cases=[c for c in prepared['cases'] if 'shading' in c['id']]
assert len(cases)==2,cases
meta={'kind':'diagnostic warm-context sampling, no comparative timing claim',
'binary_sha256':hashlib.file_digest(binary.open('rb'),'sha256').hexdigest(),'profiles':[]}
for case,mode in [(cases[0],'exact'),(cases[1],'exact'),(cases[1],'competitive')]:
    directory=out/(case['id'].replace('/','-')+'-'+mode)
    directory.mkdir()
    record={'root':str(root),'mode':mode,'case':case,'children':[]}
    meta['profiles'].append(record)
    try:
        owned_profile(binary,Path(case['pdf']['path']),Path('/usr/bin/sample'),directory,record,ProcessGroup,require_group_support)
        record['status']='completed'
    finally:
        (out/'metadata.json').write_text(json.dumps(meta,indent=2)+'\n')
print(json.dumps({'status':'completed','profiles':len(meta['profiles'])}))
