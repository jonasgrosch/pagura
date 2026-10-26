"""G01 render observations against independent source-fixture pixel oracles.

Hosted only while ADR 0031 reserves the local machine. Reuses the existing public
render adapters, but makes no timing measurement or performance claim. A complete
observation can be incorrect; it never promotes an inventory family to supported.
"""
import argparse
import json
import os
import platform
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

from capabilities import ROOT, LEDGER, local_file, read_json, require
from capability_probes import (ADAPTER_FILES, combine, observation_complete, public_identity,
                               service_commands, service_identity, sha, validate_report)
from competitors import CACHE
from benchkit.engines import command_for, engines, identify
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


def render_inventory(inventory):
    """Keep the six baseline denominator intact and add the measured subject."""
    require('pagura' not in inventory['engines'], 'subject is not a competitor baseline')
    return {**inventory, 'engines': [*inventory['engines'], 'pagura']}


def subject_identity(command):
    """Hash the actual CLI; its version alone does not attest a build revision."""
    identity = identify('pagura', command)
    if identity['status'] == 'available':
        identity['version'] = json.dumps({'engine': identity['version']})
    result = public_identity(identity)
    result['configuration'] = ('Pagura public CLI. The actual binary is hashed; '
                               'the hosted workflow records its build command and default features. '
                               'A separately supplied binary has no asserted build revision.')
    return result


def sound_observations(data):
    """A diagnostic set, separate from the oracle manifest and its pass totals."""
    controls = {'control.blank', 'annotations.display.ordinary-content',
                'annotations.display.sound-speaker-stored', 'annotations.display.sound-mic-stored'}
    cases = [case for case in data['cases'] if case['id'] in controls]
    require(len(cases) == len(controls), 'missing paired Sound drawing controls')
    cases += [{'id': 'annotations.display.sound-' + name + '-missing-ap',
               'capability': 'annotations.appearances', 'operation': 'render',
               'fixture': 'annotation_display/sound-' + name + '-missing-ap',
               'oracle': None, 'expected': None} for name in ('default', 'speaker', 'mic')]
    views = read_json(ROOT / 'corpus/manifests/annotation_display.json')['cases']
    cases += [{'id': 'annotations.display.' + row['id'].split('/', 1)[1],
               'capability': 'annotations.appearances', 'operation': 'render',
               'fixture': row['id'], 'oracle': None, 'expected': None} for row in views
              if row['id'].startswith(('annotation_display/sound-view-', 'annotation_display/text-view-'))]
    return {'schema': 1, 'cases': cases,
            'scope': 'Raw static Sound drawing observations with matched Sound/Text missing/stored AP at 72/144 DPI, '
                     'Rotate0/90 and explicit NoZoom/NoRotate flags, plus blank and ordinary-content controls. '
                     'Reader-chosen artwork has no universal pixel oracle. No pass/fail or capability promotion; '
                     'canvas-only configurations do not establish full-viewer support. No playback is requested.'}


def paired_observations(data, family):
    """Matched missing/stored annotation inputs, separate from oracle verdicts."""
    prefix, label, scope = {
        'redact': ('redact', 'Redact',
                   'Raw non-destructive Redact preview observations: matched missing/stored AP, Rect/QuadPoints, '
                   'border/interior colour, overlay text/repetition and replacement RO, with ordinary content and blank controls. '
                   'No redaction is applied, no content is removed and no replacement-phase capability is asserted. '
                   'Reader preview policy has no universal pixel oracle; no pass/fail or family promotion.'),
        'widgets': ('widget', 'Widget',
                    'Raw source Tx/Ch widget display with matched missing/stored AP and ordinary/blank controls. '
                    'Covers inherited/null values and DA, NeedAppearances, multiline/comb, choice names versus export values, '
                    'list selection/top index, widget rotation and 72/144 DPI. Original embedded Type3 glyphs isolate '
                    'font availability; values are never edited. Raw output is not an oracle verdict or full-viewer claim.'),
        'buttons': ('button', 'Button',
                    'Raw normal checkbox/radio/push-button display with paired missing/stored AP and ordinary/blank controls. '
                    'Covers AS/V precedence, normal captions, transformed icon XObjects, TP caption/icon order, all icon scaling '
                    'policies, alignment, border fitting, MK/page rotation and 72/144 DPI. Original Type3 glyphs and rectangles '
                    'isolate font/artwork availability. No values are edited, no actions or rollover/down events run, and '
                    'push buttons have no synthetic value. Reader layout observations are not oracle verdicts or full-viewer claims.'),
    }[family]
    prefix = 'annotation_display/' + prefix + '-observe-'
    cases = [case for case in data['cases'] if case['id'] == 'control.blank']
    rows = [row for row in read_json(ROOT / 'corpus/manifests/annotation_display.json')['cases']
            if row['id'].startswith(prefix)]
    identities = {row['id'] for row in rows}
    require(len(cases) == 1 and prefix + 'ordinary-content' in identities,
            'missing paired ' + label + ' drawing controls')
    require(any(name.endswith('-missing') for name in identities)
            and all(name.removesuffix('-missing') + '-stored' in identities
                    for name in identities if name.endswith('-missing')),
            'missing paired ' + label + ' stored appearance')
    cases += [{'id': 'annotations.display.' + row['id'].split('/', 1)[1],
               'capability': 'annotations.appearances', 'operation': 'render',
               'fixture': row['id'], 'oracle': None, 'expected': None} for row in rows]
    return {'schema': 1, 'cases': cases, 'scope': scope}


def fixtures(data, *, observations=False):
    """Resolve only named checked-in fixture cases, without invoking a generator."""
    rows = {}
    for area in sorted({row['fixture'].split('/')[0] for row in data['cases']}):
        require(area.isidentifier(), 'invalid fixture area')
        rows.update({row['id']: row for row in read_json(local_file(ROOT, f'corpus/manifests/{area}.json'))['cases']})
    result = {}
    for case in data['cases']:
        row = rows.get(case['fixture'])
        if row is None or row['kind'] != 'pdf' or (row['expected'] != 'render' and not observations):
            raise ValueError('fixture has no independent pixel oracle')
        if observations:
            row = {**row, 'dpi': row.get('dpi', 72)}
        require(type(row['dpi']) is int and row['dpi'] in {72, 144} and row.get('page', 0) == 0,
                'render scope requires page zero at declared 72 or 144 DPI')
        for field, digest in [('path', 'sha256'), ('oracle', 'oracle_sha256')]:
            if observations and field == 'oracle' and field not in row:
                continue
            path = local_file(ROOT, row[field])
            require(path.is_relative_to(ROOT / 'corpus/generated') and path.stat().st_size <= 64 * 1024 * 1024, 'invalid render fixture path or size')
            require(sha(path.read_bytes()) == row[digest], 'render fixture hash changed')
        if case['oracle'] == 'rgb8':
            width, height, pixels = normalize(local_file(ROOT, row['oracle']).read_bytes())
            expected = {'width': width, 'height': height, 'rgb_sha256': sha(pixels), 'tolerance': row.get('tolerance', 0)}
            require(case['expected'] == expected, 'render oracle or declared tolerance changed')
        else:
            require(observations and case['oracle'] is None and case['expected'] is None,
                    'non-oracle drawing must stay an explicit raw observation')
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


def drawing_observation(actual):
    """Raw y-down, half-open nonwhite bounds; never an artwork correctness score."""
    width, height, pixels = actual
    count, x0, y0, x1, y1 = 0, width, height, 0, 0
    for offset in range(0, len(pixels), 3):
        if pixels[offset:offset + 3] != b'\xff\xff\xff':
            y, x = divmod(offset // 3, width)
            count += 1
            x0, y0, x1, y1 = min(x0, x), min(y0, y), max(x1, x + 1), max(y1, y + 1)
    return {'width': width, 'height': height, 'rgb_sha256': sha(pixels),
            'nonwhite_pixels': count, 'nonwhite_bounds': [x0, y0, x1, y1] if count else None}


def explicit_refusal(directory, engine):
    """Only a named backend refusal; arbitrary process errors stay unclassified."""
    feature = None
    if engine == 'pagura':
        # The public CLI renders ErrorKind::Unsupported as this fixed prefix.
        match = re.match(rb'pagura: unsupported \[([A-Za-z0-9_.-]{1,96})\]: ',
                         (directory / 'stderr').read_bytes())
        if match:
            feature = match[1].decode('ascii')
    else:
        try:
            value = json.loads((directory / 'stdout').read_bytes())
            if (isinstance(value, dict) and type(value.get('schema')) is int and value['schema'] == 1
                    and value.get('status') == 'unsupported' and isinstance(value.get('feature'), str)
                    and re.fullmatch(r'[A-Za-z0-9_.-]{1,96}', value['feature'])):
                feature = value['feature']
        except (ValueError, UnicodeDecodeError):
            pass
    return feature


def invoke(command, row, directory, *, observations=False, engine=None):
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
        feature = explicit_refusal(directory, engine)
        if feature:
            return {'outcome': 'unsupported', 'feature': feature, **diagnostic}
        if observations:
            return {'outcome': 'renderer-error', 'reason': 'unclassified renderer failure; no feature verdict',
                    'exit_code': result.returncode, **diagnostic}
        return {'outcome': 'adapter-error', 'reason': 'unclassified renderer failure; requires investigation',
                'exit_code': result.returncode, **diagnostic}
    if not output.is_file() or output.stat().st_size > 64 * 1024 * 1024:
        return {'outcome': 'adapter-error', 'reason': 'missing or oversized render output', **diagnostic}
    try:
        actual = normalize(output.read_bytes())
        if observations:
            return {'outcome': 'observed', 'observed': drawing_observation(actual), **diagnostic}
        expected = normalize(local_file(ROOT, row['oracle']).read_bytes())
        verdict = verify_pixels(actual, expected, row.get('tolerance', 0))
    except ValueError:
        return {'outcome': 'adapter-error', 'reason': 'invalid raw pixel output', **diagnostic}
    observed = {'width': actual[0], 'height': actual[1], 'rgb_sha256': sha(actual[2]),
                'oracle_rgb_sha256': sha(expected[2]),
                'max_channel_error': verdict['oracle_difference'].get('max_channel_error')}
    return {'outcome': 'pass' if verdict['status'] == 'ok' else 'incorrect', 'observed': observed,
            'pixel_comparison': verdict, **diagnostic}


def run(data, inventory, output, selected, images=None, *, observations=False):
    require(selected and len(set(selected)) == len(selected) and set(selected) <= set(inventory['engines']), 'invalid selected engines')
    require(not any(os.environ.get(key) for key in ('PAGURA_HAYRO', 'PAGURA_QUARTZ', 'PAGURA_POPPLER')),
            'custom render executables require matching service/version identities')
    rows = fixtures(data, observations=True) if observations else validate_manifest(data, inventory)
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
                                                    + ' Generated annotation artwork/fonts use reader defaults; no font normalization. '
                                                    + ' Source and actual binary hashes retained.')
        identities[name] = identity
    source_files = [MANIFEST, LEDGER, *ADAPTER_FILES, *RENDER_ADAPTERS,
                    *[Path('tools') / name for name in ('capability_render.py', 'capability_probes.py', 'capabilities.py', 'competitors.py',
                                                       'benchkit/engines.py', 'benchkit/images.py', 'benchkit/config.py')],
                    Path('uv.lock'), Path('pyproject.toml'), Path('.python-version'), Path('.github/workflows/capabilities.yml'),
                    Path('benchmarks/node-package-lock.json'), Path('benchmarks/adapters/hayro/Cargo.lock'),
                    Path('benchmarks/adapters/hayro/Cargo.toml'), Path('rust-toolchain.toml'), Path('.cargo/config.toml'),
                    Path('Cargo.toml'), Path('Cargo.lock'), Path('crates/pagura-cli/Cargo.toml'),
                    Path('crates/pagura-data/Cargo.toml'), Path('data/lock.json'),
                    Path('data/afm/Helvetica.afm'), Path('data/fonts/Arimo-Regular.ttf'),
                    Path('data/afm/Helvetica-Bold.afm'), Path('data/fonts/Arimo-Bold.ttf')]
    source_files.extend(Path('corpus/manifests') / (row['fixture'].split('/')[0] + '.json') for row in data['cases'])
    report = {'schema': 1, 'scope': data['scope'], 'generated_at': datetime.now(timezone.utc).isoformat(),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'workflow_run': os.environ.get('GITHUB_RUN_ID'), 'selected_engines': list(selected),
              'command': ['python', 'tools/capability_render.py', 'observe' if observations else 'run',
                          '--engines', ','.join(selected), '--output', '$OUTPUT'],
              'environment': {'system': platform.system(), 'release': platform.release(), 'machine': platform.machine(), 'python': platform.python_version()},
              'source_sha256': {path.as_posix(): sha((ROOT / path).read_bytes()) for path in sorted(set(source_files))},
              'engines': identities, 'cases': data['cases'], 'fixtures': {}, 'results': []}
    if observations:
        report['kind'] = 'raw-render-observations'
    if images is not None:
        report['command'].extend(['--images', '$IMAGES'])
    for case in data['cases']:
        row = rows[case['id']]
        report['fixtures'][case['fixture']] = {field: row[field] for field in ('path', 'sha256', 'oracle', 'oracle_sha256', 'dpi')
                                               if field in row}
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
                    result = invoke(command_for(name, commands[name], row, directory / 'page.ppm'), row, directory,
                                    observations=observations, engine=name)
                    if images is not None and result['outcome'] in {'pass', 'incorrect', 'observed'}:
                        # Only the manifest's project-owned synthetic fixtures
                        # reach here. Keep visual evidence outside Git; names
                        # use the fixed engine and an ordinal, never PDF text.
                        folder = images / name / str(index)
                        folder.mkdir(parents=True, exist_ok=True)
                        width, height, pixels = normalize((directory / 'page.ppm').read_bytes())
                        (folder / 'actual.ppm').write_bytes(f'P6\n{width} {height}\n255\n'.encode() + pixels)
                        if 'oracle' in row:
                            (folder / 'oracle.ppm').write_bytes(local_file(ROOT, row['oracle']).read_bytes())
                        result['image_directory'] = f'{name}/{index}'
                report['results'].append({'engine': name, 'case': case['id'], **result})
    if not observations:
        validate_report(report, data, inventory)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    if observations:
        return all(row['outcome'] in {'observed', 'unsupported'} for row in report['results'] if row['engine'] in selected)
    cases = {row['id']: row for row in data['cases']}
    return all(observation_complete(row['engine'], cases[row['case']], row)
               and (row['engine'] != 'pagura' or row['outcome'] == 'pass')
               for row in report['results'] if row['engine'] in selected)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['check', 'run', 'combine', 'observe'])
    parser.add_argument('reports', type=Path, nargs='*')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--engines', default='pdfium,mupdf,pdfjs,poppler,hayro,pagura')
    parser.add_argument('--images', type=Path, help='retain synthetic actual/oracle PPMs outside Git for adjudication')
    parser.add_argument('--annotation-set', choices=['sound', 'redact', 'widgets', 'buttons'], default='sound',
                        help='raw annotation observation set; Sound remains the observe default')
    args = parser.parse_args()
    data, inventory = read_json(ROOT / MANIFEST), render_inventory(read_json(ROOT / LEDGER))
    validate_manifest(data, inventory)
    if args.command == 'check':
        print(f"G01 render manifest valid: {len(data['cases'])} independent pixel oracles; no engines invoked.")
        return
    require(args.output is not None, '--output is required')
    if args.command in {'run', 'observe'}:
        require(not args.reports, 'run does not accept report inputs')
        observations = args.command == 'observe'
        if observations:
            data = sound_observations(data) if args.annotation_set == 'sound' else paired_observations(data, args.annotation_set)
        if not run(data, inventory, args.output, tuple(args.engines.split(',')), args.images, observations=observations):
            parser.exit(1, 'Render observations incomplete or an oracle failed; inspect the saved outcomes.\n')
        if observations:
            print(f'Raw {args.annotation_set} observations recorded separately; no artwork oracle, pass total or capability verdict.')
            return
    else:
        report = combine([read_json(path) for path in args.reports], data, inventory)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    print('G01 pixel observations recorded. Incorrect results remain failures of their oracle, never parity passes.')


if __name__ == '__main__':
    main()
