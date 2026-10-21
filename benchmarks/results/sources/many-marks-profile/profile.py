#!/usr/bin/env python3
"""One preparation profile; lifecycle copied from the audited N16 profile driver."""
import argparse
import hashlib
import json
import os
from pathlib import Path
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


def owned_profile(command, sampler, directory, record, ProcessGroup, require_group_support):
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
            renderer = spawn(command, renders, errors, 'renderer')
            time.sleep(.1)
            require(alive(renderer), 'Renderer ended before the fixed profiler attachment')
            require(cancelled is None and not timed_out, 'Cancelled/expired before profiler attachment')
            command = [str(sampler), str(renderer.child.pid), '5', '1', '-mayDie', '-file', str(directory / 'sample.txt')]
            capture_start = time.monotonic_ns()
            profiler = spawn(command, sample_log, subprocess.STDOUT, 'sampler')
            profiler.wait_leader()
            record['sampler_wait_elapsed_ns'] = time.monotonic_ns() - capture_start
            record['renderer_alive_when_sampler_returned'] = alive(renderer)
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
    require(len(record['children']) == 2 and all(c.get('exit_code') == 0 for c in record['children']),
            'Renderer or sampler failed; no retry')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', type=Path, required=True)
    parser.add_argument('--plan-sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    record = {'schema': 1, 'status': 'incomplete', 'performance_claim': False,
              'children': [], 'started_unix_ns': time.time_ns()}
    sha = lambda path: hashlib.sha256(read_regular(path)).hexdigest()
    code = 1
    try:
        require(sha(args.plan) == args.plan_sha256, 'Plan changed')
        plan = json.loads(read_regular(args.plan))
        root = Path(plan['root'])
        record.update(root=str(root), plan_sha256=args.plan_sha256)
        require(plan['status'] == 'frozen-before-profile', 'Plan is not frozen')
        for path, digest in plan['files'].items():
            require(sha(Path(path)) == digest, 'File pin changed: ' + path)
        require(str(Path(__file__).resolve()) in plan['files'], 'Driver not pinned')
        sys.path.insert(0, str(root / 'tools'))
        from benchkit.config import source_digest
        from benchkit.process_group import ProcessGroup, require_group_support
        from benchkit.runner_evidence import coordination_pointer
        require(source_digest() == plan['source_sha256'], 'Product/helpers changed')
        pointer = coordination_pointer()
        require(pointer is not None, 'Runner coordination required')
        receipt = json.loads(read_regular(Path(pointer['receipt'])))
        require(receipt['status'] == 'running' and receipt['kind'] == 'profile'
                and receipt['run_id'] == pointer['run_id']
                and receipt['source_root'] == str(root)
                and receipt['source_before']['sha256'] == plan['source_sha256'],
                'Active receipt does not bind this source')
        record.update(runner=pointer, source=receipt['source_before'])
        report = json.loads(read_regular(Path(plan['conversion_report'])))
        audit = json.loads(read_regular(Path(plan['audit'])))
        summary = json.loads(read_regular(Path(plan['summary'])))
        require(audit['status'] == 'audited' and not audit['errors']
                and audit['outer_receipt_authenticated']
                and audit['report_sha256'] == sha(Path(plan['conversion_report'])), 'Reference audit differs')
        require(summary['audit']['sha256'] == sha(Path(plan['audit'])), 'Summary audit binding differs')
        for binding in summary['bindings'].values():
            bound = root / binding['receipt']
            require(sha(bound) == binding['receipt_sha256'], 'Final receipt changed')
            final = json.loads(read_regular(bound))
            require(final['exit_code'] == final['wrapper_exit_code'] == binding['exit_code']
                    and final['source_unchanged'] and not final['errors']
                    and not final['artifact_errors'], 'Final evidence receipt failed')
            for path, digest in final['artifacts'].items():
                require(sha(Path(path)) == digest, 'Bound artifact changed: ' + path)
        binary, pdf = Path(plan['binary']), Path(plan['input'])
        require(sha(binary) in report['engine_identities']['pagura']['artifacts'].values()
                and report['source_sha256'] == plan['source_sha256'], 'Reference binary/source differs')
        require(plan['files'][str(pdf)] == 'be3e7f89248060bd7ed07c885dd4aafd453db0ce99dfaf0881b9f20a93fb50c9',
                'Unexpected input bytes')
        attempts = [row for row in audit['attempts']
                    if row['engine'] == 'pagura' and row['case'] == 'classic/many-operands']
        require(len(attempts) == 42 and all(row['classification'] == 'correct' and row['binding_valid']
                and row['raw_sha256'] == sha(Path(plan['oracle'])) for row in attempts),
                'Missing independently correct reference output')
        command = [str(binary), 'microbench', str(pdf), 'render-cold', '1000', '72']
        require(plan['command'] == command, 'Fixed command differs')
        record['command'] = command
        owned_profile(command, Path('/usr/bin/sample'), directory, record, ProcessGroup, require_group_support)
        result = json.loads(read_regular(directory / 'renders.txt'))
        require(result['schema'] == 2 and result['operation'] == 'render-cold'
                and result['iterations'] == 1000 and type(result['elapsed_ns']) is int
                and result['elapsed_ns'] > 0, 'Unexpected microbench result')
        require(read_regular(directory / 'renderer-stderr.txt') == b'', 'Unexpected diagnostics')
        stack = read_regular(directory / 'sample.txt')
        pid = record['children'][0]['pid']
        require(any(line.startswith(b'Process:') and f'[{pid}]'.encode() in line
                    for line in stack[:8192].splitlines()), 'Profile does not identify owned child')
        require(source_digest() == plan['source_sha256'], 'Source changed during profile')
        for path, digest in plan['files'].items():
            require(sha(Path(path)) == digest, 'File changed during profile: ' + path)
        record.update(status='completed-diagnostic', microbench=result,
                      limitation='Sampling and instrumented elapsed time are not speed evidence. The repeated microbench black-boxes outputs; it does not independently verify each render. Same binary/input/settings have 42 independently audited RGB outputs from the frozen campaign. Requested 5-second capture is not a full-duration guarantee.')
        code = 0
    except BaseException as error:
        record['status'] = 'cancelled' if isinstance(error, Cancelled) else 'failed'
        record['error'] = f'{type(error).__name__}: {error}'
        code = 128 + error.signum if isinstance(error, Cancelled) else 1
    finally:
        record['completed_unix_ns'] = time.time_ns()
        record['artifacts_sha256'] = {p.name: sha(p) for p in directory.iterdir() if p.is_file()}
        (directory / 'record.json').write_text(json.dumps(record, indent=2) + '\n')
    return code


if __name__ == '__main__':
    raise SystemExit(main())
