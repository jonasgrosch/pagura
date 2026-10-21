"""Build and run the one temporary route test, retaining the exact test binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sys

sys.dont_write_bytecode = True
if sys.flags.optimize:
    raise ValueError('Assertions must remain enabled for this diagnostic')
ROOT = Path.cwd().resolve()
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
from paired_render import source_identity
from benchkit.config import source_digest
from benchkit.measurement import MeasurementCancelled, run_process


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('inputs', 'output', 'target', 'cargo'):
        parser.add_argument('--' + key, required=True, type=Path)
    parser.add_argument('--inputs-sha256', required=True)
    args = parser.parse_args()
    assert sha(args.inputs) == args.inputs_sha256
    inputs = json.loads(args.inputs.read_bytes())
    assert set(inputs['files']) == {'sparse64', 'opaque64', 'image-only',
        'f32-shading-dense', 'f32-pattern-dense', 'default-knockout64'}
    directory = Path(inputs['directory']).resolve()
    assert directory.is_relative_to(ROOT)
    for name, item in inputs['files'].items():
        assert Path(item['path']) == directory / (name + '.pdf')
        assert sha(item['path']) == item['sha256']
    assert os.environ.get('PAGURA_RUN_ID') and os.environ.get('PAGURA_RUN_RECEIPT')
    assert not args.target.exists()
    args.output.mkdir(parents=True, exist_ok=False)
    os.environ['PAGURA_F32_ROUTE_INPUTS'] = str(directory)
    before = source_identity(ROOT)
    report = {'schema': 1, 'status': 'incomplete', 'admission': False,
        'run_id': os.environ['PAGURA_RUN_ID'], 'receipt': os.environ['PAGURA_RUN_RECEIPT'],
        'producer_argv': sys.orig_argv, 'source_before': before,
        'runner_source_before': source_digest(), 'inputs': inputs,
        'inputs_sha256': args.inputs_sha256, 'driver_sha256': sha(__file__),
        'environment': {k: os.environ.get(k) for k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
            'CARGO_BUILD_JOBS', 'RUST_TEST_THREADS', 'PAGURA_F32_ROUTE_INPUTS')}, 'commands': []}

    def invoke(command, name):
        item = {'argv': command}
        report['commands'].append(item)
        process, log = run_process(command, {'timeout_seconds': 1200, 'measure_rss': False},
                                   log_budget=16 * 1024 * 1024)
        path = args.output / name
        path.write_text(log)
        item.update(process=process, log={'path': str(path), 'sha256': sha(path)})
        assert process['exit_code'] == 0 and process['status'] != 'timeout'
        assert len(log.encode()) < 16 * 1024 * 1024
        return log

    try:
        log = invoke([str(args.cargo), 'test', '--locked', '--offline', '--release', '-j', '1',
            '-p', 'pagura-render', '--lib', '--no-run', '--message-format=json',
            '--target-dir', str(args.target)], 'build.log')
        messages = [json.loads(line) for line in log.splitlines() if line.startswith('{')]
        artifacts = [m for m in messages if m.get('reason') == 'compiler-artifact'
            and m.get('target', {}).get('name') == 'pagura_render'
            and m.get('profile', {}).get('test') is True and m.get('executable')]
        assert len(artifacts) == 1
        source = Path(artifacts[0]['executable']).resolve()
        assert source.is_relative_to(args.target.resolve())
        binary = args.output / 'route-tests'
        shutil.copyfile(source, binary)
        binary.chmod(0o500)
        report['binary'] = {'path': str(binary), 'sha256': sha(binary), 'cargo_artifact': artifacts[0]}
        assert sha(source) == report['binary']['sha256']
        log = invoke([str(binary), 'compositor::temporary_f32_routes::temporary_f32_benchmark_routes',
            '--exact', '--nocapture', '--test-threads=1'], 'tests.log')
        report['route_rows'] = [line.split('PAGURA_F32_ROUTE_V1 ', 1)[1]
            for line in log.splitlines() if 'PAGURA_F32_ROUTE_V1 ' in line]
        assert len(report['route_rows']) == 12
        assert 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 243 filtered out;' in log
        assert sha(binary) == report['binary']['sha256']
        assert all(sha(i['path']) == i['sha256'] for i in inputs['files'].values())
        assert sha(args.inputs) == args.inputs_sha256
        report['status'] = 'verified-route-test'
    finally:
        report['source_after'] = source_identity(ROOT)
        report['runner_source_after'] = source_digest()
        if report['source_after'] != before or report['runner_source_after'] != report['runner_source_before']:
            report['status'] = 'source-changed'
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    assert report['status'] == 'verified-route-test'


if __name__ == '__main__':
    previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    def cancel(number, _frame):
        raise MeasurementCancelled(number)
    try:
        for signum in previous:
            signal.signal(signum, cancel)
        main()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
