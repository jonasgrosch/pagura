#!/usr/bin/env python3
"""One retained-product stack profile; never comparative speed/admission evidence."""
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
            renderer = spawn([str(binary), str(input_path), '0', '72', '1000'], renders, errors, 'renderer')
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
    parser.add_argument('--plan-sha256', required=True, help='independent coordinator pin')
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    args = parser.parse_args()
    directory = args.output.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    record = {'schema': 1, 'kind': 'single-retained-product-stack-profile', 'status': 'incomplete',
              'admission': False, 'performance_claim': False, 'children': [],
              'started_unix_ns': time.time_ns(), 'artifacts_sha256': {}}
    previous = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}

    def cancel(signum, _frame):
        raise Cancelled(signum)

    try:
        for signum in previous:
            signal.signal(signum, cancel)
        plan_raw = read_regular(args.plan.resolve())
        require(hashlib.sha256(plan_raw).hexdigest() == args.plan_sha256, 'Profile plan pin differs')
        plan = json.loads(plan_raw)
        require(type(plan['schema']) is int and plan['schema'] == 1
                and plan['status'] == 'frozen-before-profile' and plan['admission'] is False,
                'Plan must be filled and frozen after retention/commit')
        require(plan['configuration'] == {'case': 'opaque64', 'accuracy': 'exact', 'page': 0, 'dpi': 72,
                'reported_renders': 1000, 'existing_unreported_warmup': 1, 'attach_delay_seconds': .1,
                'sample_seconds': 5, 'sample_interval_ms': 1, 'owned_children_deadline_seconds': 120,
                'maximum_launches': 1, 'write_each_output': False}, 'Fixed profiling configuration changed')
        require(platform.system() == 'Darwin', 'This fixed plan uses macOS /usr/bin/sample')
        root = Path(plan['root']).resolve()
        record['root'] = str(root)
        sys.path.insert(0, str(root / 'benchmarks'))
        from paired_render import ROOT, source_identity
        from benchkit.artifacts import hash_regular_file, read_bounded_regular
        from benchkit.images import normalize
        from benchkit.process_group import ProcessGroup, require_group_support
        from benchkit.runner_evidence import coordination_pointer
        require(ROOT.resolve() == root, 'Imported helper checkout differs')

        def pinned_file(specification):
            path = Path(specification['path']).resolve()
            require(hash_regular_file(path) == specification['sha256'], 'File pin differs: ' + str(path))
            return path

        def pinned_json(specification):
            return json.loads(read_bounded_regular(pinned_file(specification), MAX_FILE))

        pointer = coordination_pointer()
        require(pointer is not None, 'Existing coordinator runner is required')
        start_receipt = json.loads(read_bounded_regular(Path(pointer['receipt']), MAX_FILE))
        require(start_receipt['status'] == 'running' and start_receipt['kind'] == 'benchmark'
                and start_receipt['run_id'] == pointer['run_id'] and start_receipt['source_root'] == str(root)
                and start_receipt['source_before']['commit'] == plan['source']['commit']
                and start_receipt['source_before']['sha256'] == plan['source']['runner_sha256'], 'Active runner/source differs')
        require(source_identity(root) == {'root': str(root), 'commit': plan['source']['commit'],
                'source_sha256': plan['source']['product_sha256']}, 'Retained product source differs')
        require(subprocess.check_output(['git', '-C', str(root), 'status', '--porcelain=v1', '--untracked-files=all'],
                text=True) == '', 'Profile the pristine retained commit only')
        require(hash_regular_file(Path(__file__).resolve()) == plan['driver_sha256'], 'Coordinator script changed')
        example = root / 'crates/pagura-render/examples/render_profile.rs'
        require(hash_regular_file(example) == plan['example_sha256'], 'Profile example changed')
        binary, sampler = pinned_file(plan['binary']), pinned_file(plan['sampler'])
        require(sampler == Path('/usr/bin/sample'), 'Unexpected sampler executable')
        build = pinned_json(plan['build_receipt'])
        require(build['status'] == 'completed' and build['source_unchanged'] is True
                and build['source_before'] == build['source_after'] == start_receipt['source_before']
                and build['source_root'] == str(root) and build['exit_code'] == build['wrapper_exit_code'] == 0
                and not build['errors'] and not build['artifact_errors']
                and plan['binary']['sha256'] in build['artifacts'].values(), 'Profile build lacks matching source/binary receipt')
        retention = pinned_json(plan['retention_summary'])
        require(retention['retention_decision']['decision'] == 'retain-experimentally'
                and retention['full_check']['all_checks_passed'] is True
                and retention['identities']['candidate']['source_sha256'] == plan['source']['product_sha256'],
                'No completed reviewed retention decision for this product')
        reference_path = pinned_file(plan['reference_report'])
        require(plan['reference_report']['sha256'] == retention['report_sha256'], 'Wrong retained confirmation report')
        reference = json.loads(read_bounded_regular(reference_path, MAX_FILE))
        require(reference['status'] == 'complete-exploratory' and reference['phase'] == 'confirmation'
                and reference['pairs'] == 200 and reference['warmups'] == 5
                and reference['identities']['candidate']['source_sha256'] == plan['source']['product_sha256'],
                'Reference report does not describe retained product')
        pdf, oracle_path = pinned_file(plan['input']), pinned_file(plan['oracle'])
        require(reference['inputs']['opaque64']['pdf_sha256'] == plan['input']['sha256']
                and reference['inputs']['opaque64']['oracle_sha256'] == plan['oracle']['sha256'], 'Reference input/oracle mismatch')
        cells = [c for c in reference['cells'] if (c['case'], c['mode'], c['operation']) == ('opaque64', 'exact', 'full')]
        require(len(cells) == 1, 'Missing/ambiguous reference cell')
        pairs = [p for p in cells[0]['pairs'] if p['pair'] == 0]
        require(len(pairs) == 1, 'Missing/ambiguous fixed reference pair')
        sample = pairs[0]['candidate']
        blob = PurePosixPath(sample['blob'])
        require(not blob.is_absolute() and '..' not in blob.parts, 'Unsafe reference output path')
        raw = read_bounded_regular(reference_path.parent / str(blob), MAX_FILE)
        require(hashlib.sha256(raw).hexdigest() == sample['output_sha256'], 'Reference raw output changed')
        expected = normalize(read_bounded_regular(oracle_path, MAX_FILE))
        header = b'RGBA8 2048 2048 premultiplied\n'
        require(expected[:2] == (2048, 2048) and normalize(raw) == expected and raw.startswith(header)
                and raw[len(header) + 3::4] == b'\xff' * (2048 * 2048), 'Reference pixels differ from independent oracle')
        pixels = raw[len(header):]
        checksum = fnv1a64(pixels)
        require(hashlib.sha256(pixels).hexdigest() == plan['checksum']['raw_rgba_sha256']
                and checksum == plan['checksum']['fnv1a64'], 'Expected render checksum differs')
        snapshot = directory / 'opaque64.pdf'
        with snapshot.open('xb') as handle:
            handle.write(read_bounded_regular(pdf, MAX_FILE))
        snapshot.chmod(0o400)
        with (directory / 'plan.json').open('xb') as handle:
            handle.write(plan_raw)
        with (directory / 'runner-start.json').open('x') as handle:
            json.dump(start_receipt, handle, indent=2)
        runtime_files = {str(Path(__file__).resolve()), str(Path(sys.executable).resolve()), str(example), '/bin/ps'}
        for module in tuple(sys.modules.values()):
            path = getattr(module, '__file__', None)
            if path and Path(path).is_file():
                runtime_files.add(str(Path(path).resolve()))
        manifest = {path: hash_regular_file(Path(path)) for path in sorted(runtime_files)}
        record.update(plan_sha256=args.plan_sha256, source=plan['source'], binary=plan['binary'],
                      runner={'controlled': False, 'coordination': pointer}, commit=plan['source']['commit'],
                      source_sha256=plan['source']['runner_sha256'], runtime_files_sha256=manifest,
                      python=sys.version, reference_output_sha256=sample['output_sha256'],
                      expected_rgba_fnv1a=checksum, configuration=plan['configuration'])
        require(hash_regular_file(binary) == plan['binary']['sha256']
                and hash_regular_file(snapshot) == plan['input']['sha256'], 'Invocation input changed')
        owned_profile(binary, snapshot, sampler, directory, record, ProcessGroup, require_group_support)
        lines = read_bounded_regular(directory / 'renders.txt', MAX_FILE).decode('ascii').splitlines()
        require(len(lines) == 1000, 'Missing/extra reported render rows')
        for index, line in enumerate(lines, 1):
            fields = line.split()
            require(len(fields) == 3 and fields[0] == str(index) and fields[2] == checksum,
                    'Malformed row or diagnostic checksum mismatch')
            elapsed = float(fields[1])
            require(math.isfinite(elapsed) and elapsed > 0, 'Invalid diagnostic render duration')
        require(read_bounded_regular(directory / 'renderer-stderr.txt', MAX_FILE) == b'', 'Renderer emitted diagnostics')
        stack = read_bounded_regular(directory / 'sample.txt', MAX_FILE)
        renderer_pid = record['children'][0]['pid']
        require(stack and any(line.startswith(b'Process:') and f'[{renderer_pid}]'.encode() in line
                              for line in stack[:8192].splitlines()), 'Profiler output does not identify owned renderer')
        require(source_identity(root) == {'root': str(root), 'commit': plan['source']['commit'],
                'source_sha256': plan['source']['product_sha256']}, 'Product source changed during profile')
        for path, digest in manifest.items():
            require(hash_regular_file(Path(path)) == digest, 'Code/runtime changed: ' + path)
        for specification in (plan['binary'], plan['sampler'], plan['build_receipt'], plan['retention_summary'],
                              plan['reference_report'], plan['input'], plan['oracle']):
            pinned_file(specification)
        require(hash_regular_file(snapshot) == plan['input']['sha256'], 'Owned input snapshot changed')
        record.update(status='completed-diagnostic', verified_reported_checksums=1000,
                      checksum_limitation='FNV is a diagnostic checksum, not independent byte verification of each profiled output.',
                       capture_limitation='Requested 5s capture with -mayDie; actual process lifetime and raw sample header remain evidence, not a full-duration guarantee.',
                      interpretation='Whole-process stacks include startup/open/compile, rendering, full RGBA FNV, printing and teardown. Isolate named render call chains; do not interpret whole-process sample shares or printed timings as speed evidence.')
        exit_code = 0
    except BaseException as error:
        record['status'] = 'cancelled' if isinstance(error, Cancelled) else 'failed'
        record['error'] = f'{type(error).__name__}: {error}'
        exit_code = 128 + error.signum if isinstance(error, Cancelled) else 1
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        record['completed_unix_ns'] = time.time_ns()
        try:
            for path in directory.iterdir():
                if path.name != 'record.json':
                    data = read_regular(path)
                    record['artifacts_sha256'][path.name] = hashlib.sha256(data).hexdigest()
        except BaseException as error:
            record['status'] = 'failed'
            record['retention_error'] = f'{type(error).__name__}: {error}'
            exit_code = 1
        with (directory / 'record.json').open('x') as handle:
            json.dump(record, handle, indent=2, allow_nan=False)
            handle.write('\n')
    return exit_code


if __name__ == '__main__':
    raise SystemExit(main())
