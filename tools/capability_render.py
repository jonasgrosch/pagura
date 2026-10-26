"""G01 render observations against independent source-fixture pixel oracles.

Hosted only while ADR 0031 reserves the local machine. Reuses the existing public
render adapters, but makes no timing measurement or performance claim. A complete
observation can be incorrect; it never promotes an inventory family to supported.
"""
import argparse
import json
import os
import platform
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from capabilities import ROOT, LEDGER, local_file, read_json, require
from capability_probes import (ADAPTER_FILES, combine, observation_complete,
                               service_commands, service_identity, sha, validate_report,
                               subject_identity, subject_inventory as render_inventory)
from competitors import CACHE
from benchkit.engines import command_for, engines
from benchkit.images import normalize, verify_pixels

MANIFEST = Path('benchmarks/probes/rendering.json')
RENDER_ADAPTERS = [Path('benchmarks/adapters') / name for name in
                   ('python_engine.py', 'pdfjs.mjs', 'quartz.swift', 'hayro/src/main.rs')]
RENDER_CONFIGURATIONS = {
    'pagura': 'Public render CLI; display annotation flags and stored/generated appearances.',
    'pdfium': 'pypdfium2 PdfPage.render(draw_annots=True); default colour settings.',
    'mupdf': 'PyMuPDF Page.get_pixmap(annots=True, colorspace=csRGB, alpha=False).',
    'pdfjs': 'PDFPageProxy.render(intent=display, annotationMode=ENABLE); Node canvas only, no HTML annotation layer; eval disabled.',
    'poppler': 'pdftoppm default annotation rendering; no -hide-annotations option; default colour settings.',
    'hayro': 'InterpreterSettings.render_annotations=true; default RenderSettings/PixmapSettings except requested scale.',
    'quartz': 'CoreGraphics CGContext.drawPDFPage only; no PDFKit annotation layer or substitute appearance generator.',
}


def fixtures(data):
    """Resolve only named checked-in fixture cases, without invoking a generator."""
    rows = {}
    for area in sorted({row['fixture'].split('/')[0] for row in data['cases']}):
        require(area.isidentifier(), 'invalid fixture area')
        rows.update({row['id']: row for row in read_json(local_file(ROOT, f'corpus/manifests/{area}.json'))['cases']})
    result = {}
    for case in data['cases']:
        row = rows.get(case['fixture'])
        if row is None or row['kind'] != 'pdf' or row['expected'] != 'render':
            raise ValueError('fixture has no independent pixel oracle')
        require(type(row['dpi']) is int and row['dpi'] in {72, 144} and row.get('page', 0) == 0,
                'render scope requires page zero at declared 72 or 144 DPI')
        for field, digest in [('path', 'sha256'), ('oracle', 'oracle_sha256')]:
            path = local_file(ROOT, row[field])
            require(path.is_relative_to(ROOT / 'corpus/generated') and path.stat().st_size <= 64 * 1024 * 1024, 'invalid render fixture path or size')
            require(sha(path.read_bytes()) == row[digest], 'render fixture hash changed')
        width, height, pixels = normalize(local_file(ROOT, row['oracle']).read_bytes())
        expected = {'width': width, 'height': height, 'rgb_sha256': sha(pixels), 'tolerance': row.get('tolerance', 0)}
        require(case['expected'] == expected, 'render oracle or declared tolerance changed')
        result[case['id']] = row
    return result


def validate_manifest(data, inventory):
    require(set(data) == {'schema', 'scope', 'cases'} and type(data['schema']) is int and data['schema'] == 1, 'invalid render manifest')
    require(isinstance(data['scope'], str) and data['scope'].strip(), 'missing render scope')
    require(isinstance(data['cases'], list) and data['cases'], 'empty render probes')
    identities = {row['id'] for row in inventory['capabilities']}
    seen = set()
    for case in data['cases']:
        require(set(case) == {'id', 'capability', 'operation', 'fixture', 'oracle', 'expected'}, 'invalid render case')
        require(isinstance(case['id'], str) and case['id'] and case['id'] not in seen, 'duplicate render case')
        seen.add(case['id'])
        require(case['capability'] in identities and case['operation'] == 'render' and case['oracle'] == 'rgb8', 'unassigned render operation')
        require(isinstance(case['fixture'], str) and case['fixture'].count('/') == 1, 'invalid render fixture ID')
        expected = case['expected']
        require(isinstance(expected, dict) and set(expected) == {'width', 'height', 'rgb_sha256', 'tolerance'}, 'invalid render expectation')
        require(all(type(expected[key]) is int and expected[key] > 0 for key in ('width', 'height'))
                and type(expected['tolerance']) is int and 0 <= expected['tolerance'] <= 255, 'invalid pixel dimensions or tolerance')
    require({'control.blank', 'control.rectangle'} <= seen, 'missing empty/populated render controls')
    for control, fixture in [('control.blank', 'classic/blank'), ('control.rectangle', 'classic/rectangle')]:
        require(next(row['fixture'] for row in data['cases'] if row['id'] == control) == fixture, 'changed render control fixture')
    return fixtures(data)


def invoke(command, row, directory):
    """No successful exit or stale output can stand in for independent pixels."""
    output = directory / 'page.ppm'
    output.unlink(missing_ok=True)
    with (directory / 'stdout').open('wb') as out, (directory / 'stderr').open('wb') as err:
        try:
            result = subprocess.run(command, stdout=out, stderr=err, timeout=20, cwd=ROOT)
        except subprocess.TimeoutExpired:
            return {'outcome': 'timeout', 'reason': '20 second render deadline'}
        except OSError:
            return {'outcome': 'engine-unavailable', 'reason': 'renderer could not start'}
    logs = [directory / name for name in ('stdout', 'stderr')]
    if any(path.stat().st_size > 65536 for path in logs):
        return {'outcome': 'adapter-error', 'reason': 'render diagnostics exceed 64 KiB'}
    diagnostic = {f'{path.name}_sha256': sha(path.read_bytes()) for path in logs}
    if result.returncode:
        return {'outcome': 'adapter-error', 'reason': 'unclassified renderer failure; requires investigation',
                'exit_code': result.returncode, **diagnostic}
    if not output.is_file() or output.stat().st_size > 64 * 1024 * 1024:
        return {'outcome': 'adapter-error', 'reason': 'missing or oversized render output', **diagnostic}
    try:
        actual = normalize(output.read_bytes())
        expected = normalize(local_file(ROOT, row['oracle']).read_bytes())
        verdict = verify_pixels(actual, expected, row.get('tolerance', 0))
    except ValueError:
        return {'outcome': 'adapter-error', 'reason': 'invalid raw pixel output', **diagnostic}
    observed = {'width': actual[0], 'height': actual[1], 'rgb_sha256': sha(actual[2]),
                'oracle_rgb_sha256': sha(expected[2]),
                'max_channel_error': verdict['oracle_difference'].get('max_channel_error')}
    return {'outcome': 'pass' if verdict['status'] == 'ok' else 'incorrect', 'observed': observed,
            'pixel_comparison': verdict, **diagnostic}


def run(data, inventory, output, selected, images=None):
    require(selected and len(set(selected)) == len(selected) and set(selected) <= set(inventory['engines']), 'invalid selected engines')
    require(not any(os.environ.get(key) for key in ('PAGURA_HAYRO', 'PAGURA_QUARTZ', 'PAGURA_POPPLER')),
            'custom render executables require matching service/version identities')
    rows = validate_manifest(data, inventory)
    commands, services = engines(), service_commands()
    identities = {}
    for name in inventory['engines']:
        if name not in selected:
            identities[name] = {'status': 'not-run', 'reason': 'engine assigned to another hosted platform/job'}
            continue
        identity: dict = (subject_identity(commands[name]) if name == 'pagura'
                          else service_identity(name, services[name]))
        if identity['status'] == 'available':
            def locator(value):
                return value.replace(str(ROOT), '$REPO').replace(str(CACHE), '$COMPETITORS')
            binary = Path(commands[name][0])
            if not binary.is_file():
                identity = {'status': 'unavailable', 'reason': 'render executable is missing'}
            else:
                identity['render_command'] = [locator(value) for value in commands[name]]
                identity['artifacts'].update({locator(value): sha(Path(value).read_bytes())
                                              for value in commands[name] if Path(value).is_file()})
                identity['render_configuration'] = ('Page 0 at each fixture\'s declared 72/144 DPI; white backdrop; '
                                                    + RENDER_CONFIGURATIONS[name]
                                                    + ' Source and actual binary hashes retained.')
        identities[name] = identity
    source_files = [MANIFEST, LEDGER, *ADAPTER_FILES, *RENDER_ADAPTERS,
                    *[Path('tools') / name for name in ('capability_render.py', 'capability_probes.py', 'capabilities.py', 'competitors.py',
                                                       'benchkit/engines.py', 'benchkit/images.py', 'benchkit/config.py')],
                    Path('uv.lock'), Path('pyproject.toml'), Path('.python-version'), Path('.github/workflows/capabilities.yml'),
                    Path('benchmarks/node-package-lock.json'), Path('benchmarks/adapters/hayro/Cargo.lock'),
                    Path('benchmarks/adapters/hayro/Cargo.toml'), Path('rust-toolchain.toml'), Path('.cargo/config.toml'),
                    Path('Cargo.toml'), Path('Cargo.lock'), Path('crates/pagura-cli/Cargo.toml')]
    source_files.extend(Path('corpus/manifests') / (row['fixture'].split('/')[0] + '.json') for row in data['cases'])
    report = {'schema': 1, 'scope': data['scope'], 'generated_at': datetime.now(timezone.utc).isoformat(),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'workflow_run': os.environ.get('GITHUB_RUN_ID'), 'selected_engines': list(selected),
              'command': ['python', 'tools/capability_render.py', 'run', '--engines', ','.join(selected), '--output', '$OUTPUT'],
              'environment': {'system': platform.system(), 'release': platform.release(), 'machine': platform.machine(), 'python': platform.python_version()},
              'source_sha256': {path.as_posix(): sha((ROOT / path).read_bytes()) for path in sorted(set(source_files))},
              'engines': identities, 'cases': data['cases'], 'fixtures': {}, 'results': []}
    if images is not None:
        report['command'].extend(['--images', '$IMAGES'])
    for case in data['cases']:
        row = rows[case['id']]
        report['fixtures'][case['fixture']] = {field: row[field] for field in ('path', 'sha256', 'oracle', 'oracle_sha256', 'dpi')}
    with tempfile.TemporaryDirectory(prefix='pagura-render-probes-') as temporary:
        directory = Path(temporary)
        for name in inventory['engines']:
            for index, case in enumerate(data['cases']):
                identity = identities[name]
                if name not in selected:
                    result = {'outcome': 'not-run', 'reason': identity['reason']}
                elif identity['status'] != 'available':
                    result = {'outcome': 'engine-unavailable', 'reason': identity['reason']}
                else:
                    row = rows[case['id']]
                    result = invoke(command_for(name, commands[name], row, directory / 'page.ppm'), row, directory)
                    if images is not None and result['outcome'] in {'pass', 'incorrect'}:
                        # Only the manifest's project-owned synthetic fixtures
                        # reach here. Keep visual evidence outside Git; names
                        # use the fixed engine and an ordinal, never PDF text.
                        folder = images / name / str(index)
                        folder.mkdir(parents=True, exist_ok=True)
                        width, height, pixels = normalize((directory / 'page.ppm').read_bytes())
                        (folder / 'actual.ppm').write_bytes(f'P6\n{width} {height}\n255\n'.encode() + pixels)
                        (folder / 'oracle.ppm').write_bytes(local_file(ROOT, row['oracle']).read_bytes())
                        result['image_directory'] = f'{name}/{index}'
                report['results'].append({'engine': name, 'case': case['id'], **result})
    validate_report(report, data, inventory)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    cases = {row['id']: row for row in data['cases']}
    return all(observation_complete(row['engine'], cases[row['case']], row)
               and (row['engine'] != 'pagura' or row['outcome'] == 'pass')
               for row in report['results'] if row['engine'] in selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['check', 'run', 'combine'])
    parser.add_argument('reports', type=Path, nargs='*')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--engines', default='pdfium,mupdf,pdfjs,poppler,hayro,pagura')
    parser.add_argument('--images', type=Path, help='retain synthetic actual/oracle PPMs outside Git for adjudication')
    args = parser.parse_args()
    data, inventory = read_json(ROOT / MANIFEST), render_inventory(read_json(ROOT / LEDGER))
    validate_manifest(data, inventory)
    if args.command == 'check':
        print(f"G01 render manifest valid: {len(data['cases'])} independent pixel oracles; no engines invoked.")
        return
    require(args.output is not None, '--output is required')
    if args.command == 'run':
        require(not args.reports, 'run does not accept report inputs')
        if not run(data, inventory, args.output, tuple(args.engines.split(',')), args.images):
            parser.exit(1, 'G01 render observations incomplete or Pagura missed its oracle; inspect saved outcomes.\n')
    else:
        report = combine([read_json(path) for path in args.reports], data, inventory)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    print('G01 pixel observations recorded. Incorrect results remain failures of their oracle, never parity passes.')


if __name__ == '__main__':
    main()
