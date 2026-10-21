#!/usr/bin/env python3
"""Read-only assembly of the frozen N16 correctness evidence, never a timing gate."""
import argparse
from fractions import Fraction
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import struct
import sys

sys.dont_write_bytecode = True
GATES = ('PI-Q1', 'PI-Q2', 'PI-Q3', 'PI-Q4', 'PI-Q5', 'PI-M1', 'PI-REACH')
ENCLOSING = ('Buffer', 'Option<Buffer>', 'Surface', 'MaskSink')
SEEN = {}
REVIEW = ('unchanged_layout_definitions_and_cfg', 'all_n16_writers_accounted_for',
          'dense_output_and_fallbacks_preserved', 'resource_and_error_contract_preserved',
          'diagnostic_overlays_only_and_exact_restoration', 'host_build_correspondence')
REQUIRED_TESTS = {
 'PI-Q1': ['n16_painted_interval_tracks_validated_spans_conservatively',
           'n16_fragment_interval_keeps_prior_rows_when_a_later_clip_row_fails',
           'word_output_matches_rational_rounding_for_every_word'],
 'PI-Q2': ['n16_page_output_preserves_holes_in_a_multirow_painted_interval',
           'opaque_integer_rectangles_match_irregular_and_offset_tiles_in_both_modes',
           'context_painted_then_blank_output_refills_recycled_storage_in_both_modes',
           'u8_fragment_windows_and_n16_admission_keep_their_boundaries',
           'f32_fragment_masks_keep_path_arithmetic_for_every_coverage'],
 'PI-Q3': ['direct_root_backgrounds_remain_premultiplied_in_both_modes',
           'direct_root_precision_matches_independent_normal_recurrences',
           'narrow_page_output_preserves_colored_background_across_chunk_edges'],
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def regular(path, limit=64 << 20):
    path = Path(path)
    require(path.is_absolute(), 'absolute path required')
    fd = os.open(path, os.O_RDONLY | getattr(os, 'O_NONBLOCK', 0))
    with os.fdopen(fd, 'rb') as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), 'nonregular evidence')
        data = stream.read(limit + 1)
    require(len(data) <= limit, 'evidence exceeds read bound')
    return data


def sha(path):
    fd = os.open(path, os.O_RDONLY | getattr(os, 'O_NONBLOCK', 0))
    digest, count = hashlib.sha256(), 0
    with os.fdopen(fd, 'rb') as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), 'nonregular pinned file')
        while block := stream.read(1 << 20):
            count += len(block)
            require(count <= 1 << 30, 'pinned file exceeds 1 GiB')
            digest.update(block)
    return digest.hexdigest()


def pin(item):
    require(isinstance(item, dict) and isinstance(item.get('sha256'), str), 'missing frozen pin')
    path = Path(item['path'])
    require(path.is_absolute() and sha(path) == item['sha256'], 'pin differs: ' + str(path))
    if 'bytes' in item:
        require(type(item['bytes']) is int and item['bytes'] >= 0 and path.stat().st_size == item['bytes'], 'size differs')
    require(str(path) not in SEEN or SEEN[str(path)] == item['sha256'], 'conflicting file pins')
    SEEN[str(path)] = item['sha256']
    return path


def raw(item, limit=64 << 20):
    data = regular(pin(item), limit)
    require(hashlib.sha256(data).hexdigest() == item['sha256'], 'file changed during read')
    return data


def load(item):
    return json.loads(raw(item))


def pins(mapping):
    require(isinstance(mapping, dict) and mapping, 'empty pin closure')
    for path, digest in mapping.items():
        pin({'path': path, 'sha256': digest})


def module(name, item):
    path = pin(item)
    sys.path.insert(0, str(path.parent))
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def process(value):
    require(type(value.get('exit_code')) is int and value['exit_code'] == 0
            and value.get('status') != 'timeout', 'nonzero/timeout process')


def successful(receipt):
    return (receipt.get('status') == 'completed' and receipt.get('source_unchanged') is True
            and receipt.get('source_before') == receipt.get('source_after')
            and not receipt.get('errors') and not receipt.get('artifact_errors')
            and receipt.get('cancellation_signal') is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0 for k in ('exit_code', 'wrapper_exit_code')))


def job(item, *, passing=True):
    receipt, guard, report = (load(item[k]) for k in ('receipt', 'guard', 'report'))
    require(receipt['argv'] == item['receipt_argv'] and guard['command'] == item['guard_command'], 'job launch chain differs')
    require(receipt['source_root'] == item['source_root'] and receipt['cwd'] == item['source_root'], 'job root differs')
    require(receipt['source_before'] == receipt['source_after'] == item['runner_source'], 'job source differs')
    for key in ('report', 'guard', *item.get('receipt_artifacts', [])):
        descriptor = item[key]
        pin(descriptor)
        require(receipt['artifacts'].get(descriptor['path']) == descriptor['sha256'], 'final receipt lacks artifact')
    snapshots = item['snapshots']
    require(set(snapshots) == {event['snapshot'] for event in guard['events']}, 'snapshot inventory differs')
    for event in guard['events']:
        name = event['snapshot']
        require(Path(name).name == name and snapshots[name]['path'] == str(Path(item['guard']['path']).parent / name), 'unsafe snapshot path')
        require(raw(snapshots[name]), 'empty process snapshot')
        original = receipt['artifacts'].get(snapshots[name]['path'])
        require(original is None or original == snapshots[name]['sha256'], 'original snapshot hash differs')
    if report.get('run_id') is not None:
        require(report['run_id'] == receipt['run_id'], 'report run differs')
    if 'producer_argv' in report:
        require(report['producer_argv'] == item['producer_argv'], 'actual helper argv differs')
    if passing:
        require(successful(receipt), 'unsuccessful final receipt')
        require(guard.get('status') == 'completed' and guard.get('observation_only') is False
                and not guard.get('cancellation_requested_utc') and guard.get('events')
                and all(type(guard.get(k)) is int and guard[k] == 0 for k in ('exit_code', 'child_exit_code'))
                and all(event.get('matches') == [] for event in guard['events'])
                and {'preflight', 'postflight'} <= {event['stage'] for event in guard['events']}, 'strict observer not completed cleanly')
    return report, receipt, guard


def artifacts(report, directory):
    for name, item in report['artifacts'].items():
        path = directory / name
        require(not Path(name).is_absolute() and path.resolve().is_relative_to(directory.resolve()), 'unsafe artifact path')
        pin({'path': str(path), **item})


def text_log(item):
    data = raw(item, 16 << 20)
    require(len(data) < 16 << 20, 'log reached capture bound')
    text = data.decode('utf-8')
    require('\ufffd' not in text, 'replacement character in captured log')
    return text


def stage_check(item, helper, root, source):
    plan, aggregate = load(item['plan']), load(item['aggregate'])
    require(aggregate['status'] == 'verified-staged-check' and aggregate['stage_count'] == 14
            and aggregate['source'] == plan['source'] == source
            and aggregate['plan_sha256'] == item['plan']['sha256'], 'staged aggregate differs')
    helper.read, helper.sha, helper.source_digest = regular, sha, source_digest
    helper.verify(plan, root)
    inventory = aggregate['artifacts_sha256']
    require(set(inventory) == {str(p) for p in Path(plan['attempts']).rglob('*') if p.is_file()}, 'attempt inventory changed')
    pins(inventory)
    actual = []
    for expected in aggregate['attempts']:
        directory = Path(plan['attempts']) / expected['attempt']
        def retained(relative):
            path = str(directory / relative)
            return load({'path': path, 'sha256': inventory[path]})
        receipt, guard = retained('receipt.json'), retained('observer/guard.json')
        report_path = directory / 'stage/report.json'
        report = retained('stage/report.json') if report_path.exists() else None
        index = expected['index']
        argv = helper.command(plan, Path(item['plan']['path']), item['plan']['sha256'], index, directory.name)
        require(guard['command'] == argv and receipt['argv'] == [plan['python'], plan['guard'], '--evidence-dir', str(directory / 'observer'), '--', *argv], 'stage argv differs')
        require(receipt['source_root'] == receipt['cwd'] == str(root), 'stage root differs')
        needed = [directory / 'observer/guard.json']
        if report:
            require(report['index'] == index and report['plan_sha256'] == item['plan']['sha256']
                    and report['producer_argv'] == argv and report['receipt'] == str(directory / 'receipt.json'), 'stage report binding differs')
            needed.append(report_path)
            if report.get('log'):
                require(report['log']['path'] == str(directory / 'stage/stage.log'), 'stage log path differs')
                text_log(report['log']); needed.append(directory / 'stage/stage.log')
        for path in needed:
            require(receipt['artifacts'].get(str(path)) == inventory[str(path)], 'stage receipt artifact differs')
        for event in guard['events']:
            name = event['snapshot']
            require(Path(name).name == name, 'unsafe stage snapshot')
            path = str(directory / 'observer' / name)
            require(path in inventory and regular(path), 'missing stage snapshot')
            require(path not in receipt['artifacts'] or receipt['artifacts'][path] == inventory[path], 'original stage snapshot differs')
        row = dict(expected, report=report, receipt=receipt, guard=guard)
        require(row == expected, 'aggregate embedded attempt differs from retained bytes')
        actual.append(row)
    helper.decide(plan, actual)


def diagnostics(item, helper, identities):
    report, _, _ = job(item)
    preparation, restored = load(item['preparation']), load(item['restoration'])
    require(report['status'] == 'verified-tests-and-diagnostics' and report['layout_gate_passed'] is True
            and report['expected_builds'] == report['expected_test_runs'] == 6, 'diagnostic incomplete')
    require(report['preparation_sha256'] == item['preparation']['sha256'] == restored['preparation_sha256'], 'preparation link differs')
    require(restored['restored'] is True and restored['removed'] == identities, 'diagnostic restoration differs')
    require({side: row['before'] for side, row in preparation['roots'].items()} == identities, 'diagnostic production basis differs')
    require(report['source_before'] == report['source_after'] == {side: row['installed'] for side, row in preparation['roots'].items()}, 'installed diagnostic source differs')
    for side, row in preparation['roots'].items():
        pins({str(Path(identities[side]['root']) / name): digest for name, digest in row['files'].items()})
    require(report['fixtures_before'] == report['fixtures_after'] and report['fixtures_before']['baseline']
            and report['fixtures_before']['baseline'] == report['fixtures_before']['candidate'], 'diagnostic fixtures differ')
    for side, mapping in report['fixtures_before'].items():
        pins({str(Path(identities[side]['root']) / name): digest for name, digest in mapping.items()})
    pins(report['bound_files_sha256'])
    directory = Path(item['report']['path']).parent
    artifacts(report, directory)
    require(raw(item['parser']) == regular(directory / 'code/run.py'), 'executed diagnostic parser differs')
    commands = {row['log']: row for row in report['commands']}
    require(len(commands) == len(report['commands']), 'duplicate command logs')
    for command in commands.values():
        require(command['status'] == 'returned', 'diagnostic command incomplete'); process(command['process'])
        text_log({'path': str(directory / command['log']), **report['artifacts'][command['log']]})
    seen = {}
    for observation in report['observations']:
        side, target = observation['arm'], observation['platform']
        require((side, target) not in seen and side in helper.ARMS and target in helper.PLATFORMS, 'unexpected diagnostic arm')
        seen[side, target] = observation
        prefix = f'{side}/{target}'
        log = text_log({'path': str(directory / (prefix + '-tests.log')), **report['artifacts'][prefix + '-tests.log']})
        parsed = helper.parse_tests(log, side, target)
        require(all(observation[key] == value for key, value in parsed.items()), 'raw diagnostic parsing differs')
        if side == 'candidate':
            names = {name.rsplit('::', 1)[-1] for name in parsed['test_names']}
            require(all(set(required) <= names for required in REQUIRED_TESTS.values()), 'named accuracy assertion missing')
        pin({'path': observation['binary'], 'sha256': observation['binary_sha256']})
        build = commands[prefix + '-build.log']
        build_log = text_log({'path': str(directory / build['log']), **report['artifacts'][build['log']]})
        messages = [json.loads(line) for line in build_log.splitlines() if line.startswith('{')]
        finished = [row for row in messages if row.get('reason') == 'build-finished']
        candidates = [row for row in messages if row.get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == 'pagura_render' and row['target'].get('kind') == ['lib'] and row.get('profile', {}).get('test') is True and row.get('executable')]
        require(len(finished) == len(candidates) == 1 and finished[0]['success'] is True and candidates[0] == observation['cargo_artifact'], 'Cargo artifact correspondence differs')
        require(build['command'] == item['build_commands'][prefix] and build['cwd'] == identities[side]['root'], 'diagnostic build command differs')
        require(build['rustflags'] == (None if target == 'native' else '-C target-feature=' + ('+simd128' if target == 'wasi-simd' else '-simd128')), 'diagnostic flags differ')
        test = commands[prefix + '-tests.log']
        require(test['cwd'] == identities[side]['root'] and test['command'] == item['test_commands'][prefix], 'diagnostic test invocation differs')
        if target != 'native':
            metadata = [json.loads(line) for line in log.splitlines() if line.startswith('{"schema":')]
            require(metadata == [observation['wasi']] and metadata[0]['module'] == {'path': observation['binary'], 'sha256': observation['binary_sha256']}
                    and metadata[0]['test_args'] == helper.TEST_ARGS and metadata[0]['runtime']['exec_argv'] == []
                    and metadata[0]['preopens'] == {identities[side]['root']: identities[side]['root']}, 'WASI invocation metadata differs')
    require(set(seen) == {(side, target) for side in helper.ARMS for target in helper.PLATFORMS}, 'six arms missing')
    for target in helper.PLATFORMS:
        left, right = (seen[side, target] for side in helper.ARMS)
        require(left['pi_q4'] == right['pi_q4'], 'same-platform Q4 differs')
        for name in helper.TYPES:
            for key in ('size', 'align'):
                a, b = left['layouts'][name][key], right['layouts'][name][key]
                require(type(a) is int and type(b) is int and 0 < b <= a, 'layout growth/invalid observation')
    rows = []
    for target, observed in (('native', 'native'), ('wasm32-wasip1', 'wasi-simd')):
        for name in ENCLOSING:
            row = {'target': target, 'type': name}
            for side in helper.ARMS:
                value = seen[side, observed]['layouts'][name]
                if target != 'native':
                    require(value == seen[side, 'wasi-scalar']['layouts'][name], 'SIMD/scalar layout differs')
                row[side] = {'size': value['size'], 'alignment': value['align']}
            rows.append(row)
    return rows


def framed(raw_bytes):
    frames, offset = [], 0
    while offset < len(raw_bytes):
        require(offset + 4 <= len(raw_bytes), 'partial frame prefix')
        length, = struct.unpack_from('<I', raw_bytes, offset); offset += 4
        require(5 <= length <= 1 << 20 and offset + length <= len(raw_bytes), 'partial/oversize host frame')
        body = raw_bytes[offset:offset + length]; offset += length
        frames.append((body[0], struct.unpack_from('<I', body, 1)[0], body[5:]))
    return frames


def precision(item, identities, source):
    report, _, _ = job(item)
    require(report['status'] == 'passed' and report['root'] == identities['candidate']['root']
            and report['source_before'] == report['source_after']
            and report['source_before']['head'] == source['commit'], 'host source/status differs')
    root, directory = Path(report['root']), Path(item['report']['path']).parent
    pins({str(root / name): digest for name, digest in report['source_before']['files'].items()})
    pins({str(root / name): digest for name, digest in report['protocol'].items()})
    artifacts(report, directory)
    for target in ('native', 'wasi'):
        build, receipt, _ = job(item['builds'][target])
        process(build['process'])
        require(build['status'] == 'command-passed' and build['source_before'] == build['source_after'] == source['sha256']
                and build['command'] == item['builds'][target]['command'], 'host build source/command differs')
        descriptor = item['builds'][target]['log']
        require(build['log_sha256'] == descriptor['sha256'] and receipt['artifacts'].get(descriptor['path']) == descriptor['sha256'], 'host build log binding differs')
        messages = [json.loads(line) for line in text_log(descriptor).splitlines() if line.startswith('{')]
        finished = [row for row in messages if row.get('reason') == 'build-finished']
        binaries = [row['executable'] for row in messages if row.get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == 'pagura-host' and row.get('executable')]
        require(len(finished) == 1 and finished[0]['success'] is True and binaries == [report['binaries'][target]['path']], 'host build output differs')
        pin(report['binaries'][target])
        require(receipt['artifacts'].get(report['binaries'][target]['path']) == report['binaries'][target]['sha256'], 'final host build receipt lacks exact binary')
    require(len(report['cases']) == 2, 'host case inventory')
    for case, stem, truth in zip(report['cases'], ('precision-root-two-partial', 'precision-root-slivers-1000'), (Fraction(44617, 204), 255 * Fraction(254, 255) ** 1000)):
        require(case['id'] == 'content/' + stem and case['status'] == 'passed' and case['diagnostics'] == []
                and case['raw_response_parity'] is True, 'host case failed')
        require(type(case['native']['status']) is int and case['native']['status'] == 0 and case['native']['signal'] is None
                and case['native']['error'] is None and case['native']['command'] == [report['binaries']['native']['path']], 'native host failed')
        require(case['wasi'] == {'exit_code': 0, 'args': [], 'env': {}, 'preopens': {}}, 'WASI host context differs')
        pdf = regular(directory / (stem + '.pdf'))
        require(hashlib.sha256(pdf).hexdigest() == case['input_sha256']
                and pdf == regular(root / 'corpus/generated/content' / (stem + '.pdf')), 'host PDF differs')
        request = regular(directory / (stem + '.request.bin'))
        require(hashlib.sha256(request).hexdigest() == case['request_sha256'], 'host request differs')
        require(framed(request) == [(1, 1, struct.pack('<I', 0x10004)),
            (2, 2, struct.pack('<BII', 0, 0, len(pdf)) + pdf), (3, 3, struct.pack('<II', 1, 0)),
            (4, 4, struct.pack('<IIdIIII', 1, 0, 1, 0, 0, 1, 1)),
            (5, 5, struct.pack('<I', 1)), (17, 6, struct.pack('<I', 1)), (6, 7, struct.pack('<I', 1))], 'host strict request differs')
        native = regular(directory / (stem + '.native.response.bin'))
        require(native == regular(directory / (stem + '.wasi.response.bin')), 'raw host response parity differs')
        frames = framed(native)
        require([(kind, number) for kind, number, _ in frames] == list(zip((129, 130, 131, 132, 133, 145, 134), range(1, 8))), 'host response inventory')
        payloads = [data for _, _, data in frames]
        require(payloads[0] == struct.pack('<I', 0x10004) and payloads[1] == struct.pack('<II', 1, 1)
                and payloads[2] == struct.pack('<4dHdII', 0, 0, 1, 1, 0, 1, 1, 1)
                and payloads[4] == payloads[5] == bytes(4) and payloads[6] == b'', 'host geometry/diagnostics differs')
        require(len(payloads[3]) == 12 and payloads[3][:8] == struct.pack('<II', 1, 1), 'host tile shape')
        rgba = list(payloads[3][8:])
        require(rgba == case['rgba'] and rgba[3] == 255 and all(abs(Fraction(byte) - truth) <= 1 for byte in rgba[:3]), 'independent rational host error')


def reach(item, helper, identities, source, prior):
    report, _, _ = job(item)
    preparation, restored = load(item['preparation']), load(item['restoration'])
    require(report['schema'] == 2 and report['revision'] == 'R2' and report['status'] == 'verified-route-reach'
            and report['expected_outputs'] == report['completed_outputs'] == len(report['records']) == 24
            and report['all_independent_rgba'] is True and report['all_route_expectations'] is True, 'R2 not complete')
    require(report['prior_failed_reach'] == preparation['prior_failed_reach'] == prior, 'R1 failure link differs')
    require(report['preparation_sha256'] == item['preparation']['sha256']
            and report['pre_probe_source'] == preparation['pre_probe_source'] == identities['candidate']
            and report['instrumented_source'] == report['source_after'] == preparation['instrumented_source'], 'R2 source binding differs')
    require(restored['schema'] == 2 and restored['revision'] == 'R2' and restored['restored'] is True
            and restored['source_after'] == restored['pre_probe_source'] == identities['candidate']
            and restored['runner_source_after'] == source and restored['helper_closure_unchanged'] is True
            and restored['files_before'] == restored['files_after'] == preparation['files_before']
            and restored['files_installed'] == preparation['files_installed'], 'R2 restoration differs')
    pin(restored['preparation'])
    pins({str(Path(identities['candidate']['root']) / name): digest for name, digest in restored['files_after'].items()})
    require(load(report['retained_preparation']) == preparation and load(report['retained_request_plan']) == json.loads(json.dumps(helper.frozen_plan())), 'retained R2 preparation/request plan differs')
    for descriptor in report['retained_build_and_patch']:
        pin(descriptor)
    pins(report['harness_runtime_sha256'])
    require(report['binary'] == preparation['binary'] and report['binary_sha256_after'] == report['binary']['sha256'], 'R2 binary drift')
    pin(report['binary'])
    build, build_receipt, _ = job(item['build'])
    require(build['status'] == 'command-passed' and build['source_before'] == build['source_after'] == item['build']['runner_source']['sha256'], 'R2 build source differs')
    process(build['process'])
    require(build['command'] == item['build']['command'], 'R2 build command differs')
    build_log = text_log(item['build']['log'])
    require(build['log_sha256'] == item['build']['log']['sha256'], 'R2 build log differs')
    for descriptor in preparation['build_evidence']:
        pin(descriptor)
    require(item['build']['receipt'] in preparation['build_evidence'], 'R2 preparation misses final build receipt')
    messages = [json.loads(line) for line in build_log.splitlines() if line.startswith('{')]
    finished = [row for row in messages if row.get('reason') == 'build-finished']
    outputs = [row.get('executable') for row in messages if row.get('reason') == 'compiler-artifact'
               and row.get('target', {}).get('name') == 'scene_index_profile' and row.get('executable')]
    require(len(finished) == 1 and finished[0]['success'] is True and outputs == [report['binary']['path']]
            and build_receipt['artifacts'].get(report['binary']['path']) == report['binary']['sha256'], 'R2 build output binding differs')
    require(set(report['inputs']) == {request[1] for request in helper.REQUESTS}, 'R2 input inventory differs')
    records = iter(report['records'])
    for request in helper.REQUESTS:
        name, fixture, dimension, *_ = request
        inputs = report['inputs'][fixture]
        require(inputs == item['inputs'][fixture], 'R2 independently frozen fixture pins differ')
        pin(inputs['pdf']); pixels = helper.oracle_pixels(raw(inputs['oracle']), dimension)
        for mode in ('exact', 'competitive'):
            record = next(records)
            background = [17, 85, 170, 128] if name == 'nonopaque-empty' else [255] * 4
            expected = {'request': name, 'fixture': fixture, 'mode': mode, 'operation': 'full', 'dpi': 72,
                        'background': background, 'pdf_sha256': inputs['pdf']['sha256'], 'oracle_sha256': inputs['oracle']['sha256']}
            require(all(record[key] == value for key, value in expected.items()), 'R2 request identity differs')
            command = [report['binary']['path'], inputs['pdf']['path'], mode, 'full', record['output']['path'], '72']
            if name == 'nonopaque-empty':
                command.append('17,85,170,128')
            require(record['command'] == command and record['independent_rgba'] is True and record['reach_passed'] is True, 'R2 invocation failed')
            process(record['process'])
            helper.verify_output(raw(record['output']), pixels, dimension, name == 'nonopaque-empty')
            log = text_log(record['log'])
            parsed = helper.trace(log, request, mode, background)
            require(parsed == (record['plan'], record['chunks'], record['marker_count']), 'R2 raw trace differs')
            stages = [json.loads(line) for line in log.splitlines() if line.startswith('{')]
            require(stages == [record['raw_stages_do_not_benchmark']] and all(stages[0].get(key) == value for key, value in {'schema': 1, 'operation': 'full', 'accuracy': mode, 'dpi': 72, 'width': dimension, 'height': dimension}.items()), 'R2 stage identity differs')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', required=True, type=Path)
    parser.add_argument('--manifest-sha256', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    require(args.output.is_absolute() and not args.output.exists(), 'new absolute quality output required')
    manifest_pin = {'path': str(args.manifest), 'sha256': args.manifest_sha256}
    manifest = load(manifest_pin)
    root = Path(manifest['root'])
    require(manifest['schema'] == 1 and manifest['experiment'] == 'n16-painted-interval'
            and root.is_absolute() and Path.cwd() == root, 'manifest/root differs')
    pins(manifest['files_sha256'])
    require(manifest['files_sha256'].get(str(Path(__file__).resolve())) == sha(__file__)
            and all(str(Path(path).resolve()) in manifest['files_sha256'] for path in (sys.executable, sys.orig_argv[0])), 'aggregator/runtime closure unpinned')
    sys.path[:0] = [str(root / 'benchmarks'), str(root / 'tools')]
    global source_digest
    from benchkit.config import source_digest
    from paired_render import source_identity
    identity = source_identity(root)
    source = {'commit': identity['commit'], 'sha256': source_digest()}
    require(identity == manifest['identities']['candidate'] and source == manifest['source'], 'production source drift')
    require(source_identity(Path(manifest['identities']['baseline']['root'])) == manifest['identities']['baseline'], 'baseline source drift')
    live_path, run_id = os.environ.get('PAGURA_RUN_RECEIPT'), os.environ.get('PAGURA_RUN_ID')
    require(live_path and run_id, 'aggregation must run under coordinator')
    live = json.loads(regular(live_path))
    expected_argv = [args.manifest_sha256 if value == '{manifest_sha256}' else value for value in manifest['aggregator_argv']]
    require(live['status'] == 'running' and live['run_id'] == run_id and live['source_root'] == str(root)
            and live['source_before'] == source and sys.orig_argv == expected_argv, 'live aggregation receipt/argv differs')
    report = {'schema': 1, 'experiment': 'n16-painted-interval', 'status': 'incomplete', 'admission': False,
              'source': source, 'run_id': run_id, 'manifest': manifest_pin, 'gates': {gate: 'pending' for gate in GATES},
              'layouts': [], 'attempts': [], 'limitations': manifest['limitations']}
    def save():
        temporary = args.output.with_suffix('.pending')
        temporary.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
        temporary.replace(args.output)
    def cancel(number, _frame):
        raise InterruptedError('aggregation cancelled by signal ' + str(number))
    handlers = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        for number in handlers:
            signal.signal(number, cancel)
        save()
        staged = module('quality_staged', manifest['helpers']['staged'])
        diagnostic = module('quality_diagnostic', manifest['helpers']['diagnostics'])
        reach_helper = module('quality_reach', manifest['helpers']['reach'])
        stage_check(manifest['staged'], staged, root, source)
        report['layouts'] = diagnostics(manifest['diagnostics'], diagnostic, manifest['identities'])
        precision(manifest['precision'], manifest['identities'], source)
        failed = manifest['failed_attempts']
        load(manifest['historical_ledger'])
        require(failed and any(item['id'] == 'reach-001-R1' for item in failed), 'R1 failed attempt missing')
        for item in failed:
            old = load(item['report'])
            require(old.get('status') == item['status'] and item['reason'], 'failed attempt classification differs')
            for descriptor in item['evidence']:
                pin(descriptor)
            report['attempts'].append(item)
        prior = next(item['report'] for item in failed if item['id'] == 'reach-001-R1')
        require(load(prior).get('status') != 'verified-route-reach', 'R1 must remain failed')
        reach(manifest['reach'], reach_helper, manifest['identities'], source, prior)
        review = manifest['source_review']
        require(raw(review['artifact']), 'empty source-review attestation')
        require(review['source'] == source and review['status'] == 'reviewed' and set(review['checks']) == set(REVIEW)
                and all(value is True for value in review['checks'].values()) and review['notes'], 'source review incomplete')
        pins(review['files_sha256'])
        require(source_identity(root) == identity and {'commit': identity['commit'], 'sha256': source_digest()} == source, 'production changed during assembly')
        for loaded in tuple(sys.modules.values()):
            path = getattr(loaded, '__file__', None)
            if path and Path(path).resolve().is_relative_to(root):
                resolved = str(Path(path).resolve())
                require(manifest['files_sha256'].get(resolved) == sha(resolved), 'loaded helper missing from frozen closure')
        require(all(sha(path) == digest for path, digest in SEEN.items()), 'evidence changed during assembly')
        report.update(status='passed', gates={gate: 'passed' for gate in GATES}, source_review=manifest['source_review'],
                      evidence={key: manifest[key] for key in ('staged', 'diagnostics', 'precision', 'reach')},
                      historical_ledger=manifest['historical_ledger'],
                      verified_files_sha256=SEEN,
                      scope='Complete staged authoritative check and separately authenticated correctness diagnostics; aggregation only, no performance/admission result.')
    except BaseException as error:
        report.update(status='failed-or-incomplete', gates={gate: 'pending' for gate in GATES}, error=f'{type(error).__name__}: {error}')
        raise
    finally:
        try:
            save()
        finally:
            for number, handler in handlers.items():
                signal.signal(number, handler)


if __name__ == '__main__':
    main()
