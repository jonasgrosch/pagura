"""Negative controls for pixel observations; never import or invoke a PDF engine."""
import copy
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import capability_render as render
from capability_probes import pixel_pass


class RenderProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = render.read_json(render.ROOT / render.MANIFEST)
        cls.inventory = render.read_json(render.ROOT / render.LEDGER)

    def test_subject_is_additive_and_all_seven_columns_are_required(self):
        inventory = render.render_inventory(self.inventory)
        self.assertNotIn('pagura', self.inventory['engines'])
        self.assertEqual(len(inventory['engines']), len(self.inventory['engines']) + 1)
        self.assertEqual(inventory['engines'][:-1], self.inventory['engines'])
        with self.assertRaisesRegex(ValueError, 'not a competitor'):
            render.render_inventory(inventory)
        # A report that omits the subject cannot be silently reused as a
        # comparison against it, even when all six competitors were measured.
        report = {'schema': 1, 'cases': self.manifest['cases'],
                  'engines': dict.fromkeys(self.inventory['engines'], {'status': 'available'})}
        with self.assertRaisesRegex(ValueError, 'lost a baseline'):
            render.validate_report(report, self.manifest, inventory)

    def test_subject_identity_never_guesses_binary_revision_or_prints_raw_errors(self):
        identity = {'status': 'available', 'version': 'pagura 0.1.0 (safe Rust)',
                    'command': [str(render.ROOT / 'target/release/pagura')],
                    'artifacts': {str(render.ROOT / 'target/release/pagura'): 'a' * 64}}
        with patch.object(render, 'identify', return_value=identity):
            actual = render.subject_identity(['stub'])
        self.assertEqual(actual['version'], {'engine': 'pagura 0.1.0 (safe Rust)'})
        self.assertEqual(actual['artifacts'], {str(Path('$REPO') / 'target/release/pagura'): 'a' * 64})
        self.assertNotIn('revision', actual)
        with patch.object(render, 'identify', return_value={'status': 'unavailable', 'reason': '/private/path'}):
            actual = render.subject_identity(['stub'])
        self.assertEqual(actual['status'], 'unavailable')
        self.assertNotIn('/private/path', str(actual))

    def test_manifest_cannot_change_tolerance_or_drop_empty_control(self):
        render.validate_manifest(self.manifest, self.inventory)
        changed = copy.deepcopy(self.manifest)
        changed['cases'][0]['expected']['tolerance'] = 255
        with self.assertRaisesRegex(ValueError, 'tolerance changed'):
            render.validate_manifest(changed, self.inventory)
        changed = copy.deepcopy(self.manifest)
        changed['cases'].pop(0)
        with self.assertRaisesRegex(ValueError, 'controls'):
            render.validate_manifest(changed, self.inventory)

    def test_pass_requires_dimensions_hashes_and_bounded_difference(self):
        case = self.manifest['cases'][0]
        expected = case['expected']
        observed = {'width': expected['width'], 'height': expected['height'],
                    'rgb_sha256': expected['rgb_sha256'], 'oracle_rgb_sha256': expected['rgb_sha256'],
                    'max_channel_error': 0}
        self.assertTrue(pixel_pass(case, observed))
        for key, value in [('width', 1), ('height', True), ('rgb_sha256', 'a' * 64),
                           ('oracle_rgb_sha256', 'b' * 64), ('max_channel_error', None),
                           ('max_channel_error', 1), ('max_channel_error', -1), ('max_channel_error', True)]:
            with self.subTest(key=key, value=value):
                self.assertFalse(pixel_pass(case, {**observed, key: value}))

    def test_declared_resolution_reaches_each_adapter_without_resizing(self):
        rows = render.validate_manifest(self.manifest, self.inventory)
        row = rows['annotations.circle-fill-144']
        self.assertEqual((row['dpi'], row['width'], row['height']), (144, 64, 64))
        for name in render.render_inventory(self.inventory)['engines']:
            command = render.command_for(name, ['adapter'], row, Path('/tmp/probe.ppm'))
            if name == 'poppler':
                self.assertEqual(command[command.index('-r') + 1], '144')
            else:
                self.assertEqual(command[-2:], ['144', '0'])
        original_read = render.read_json
        def changed_read(path):
            data = original_read(path)
            if path.name == 'circle_annotations.json':
                for case in data['cases']:
                    case['dpi'] = invalid
            return data
        for invalid in (True, 0, 96, 288):
            with patch.object(render, 'read_json', side_effect=changed_read), self.assertRaisesRegex(ValueError, 'declared'):
                render.validate_manifest(self.manifest, self.inventory)

    def test_stale_success_wrong_pixels_crash_and_timeout_are_distinct(self):
        row = render.validate_manifest(self.manifest, self.inventory)['control.rectangle']
        correct = (render.ROOT / row['oracle']).read_bytes()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            output = directory / 'page.ppm'
            output.write_bytes(correct)
            with patch.object(render.subprocess, 'run', return_value=subprocess.CompletedProcess(['stub'], 0)):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'adapter-error')
            def pixels(data):
                def stub(command, **_):
                    output.write_bytes(data)
                    return subprocess.CompletedProcess(command, 0)
                return stub
            with patch.object(render.subprocess, 'run', side_effect=pixels(correct)):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'pass')
            wrong = correct[:-1] + bytes([correct[-1] ^ 255])
            with patch.object(render.subprocess, 'run', side_effect=pixels(wrong)):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'incorrect')
            with patch.object(render.subprocess, 'run', side_effect=pixels(b'not pixels')):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'adapter-error')
            with patch.object(render.subprocess, 'run', return_value=subprocess.CompletedProcess(['stub'], -11)):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'adapter-error')
            with patch.object(render.subprocess, 'run', side_effect=subprocess.TimeoutExpired('stub', 20)):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'timeout')
            with patch.object(render.subprocess, 'run', side_effect=FileNotFoundError):
                self.assertEqual(render.invoke(['stub'], row, directory)['outcome'], 'engine-unavailable')

    def test_reader_artwork_is_a_raw_observation_with_paired_controls(self):
        data = render.sound_observations(self.manifest)
        rows = render.fixtures(data, observations=True)
        subjects = [case for case in data['cases'] if case['fixture'].endswith('-missing-ap')]
        self.assertEqual(len(subjects), 3)
        self.assertEqual(len(rows), 33)
        for case in subjects:
            self.assertIsNone(case['expected'])
            self.assertNotIn('oracle', rows[case['id']])
            self.assertEqual(rows[case['id']]['dpi'], 72)
        self.assertEqual(rows['annotations.display.sound-view-144-90-28-missing']['dpi'], 144)
        self.assertEqual(rows['annotations.display.text-view-72-90-4-stored']['dpi'], 72)
        with self.assertRaises(ValueError):
            render.validate_manifest(data, self.inventory)
        with self.assertRaisesRegex(ValueError, 'report/manifest mismatch'):
            render.combine([{'schema': 1, 'kind': 'raw-render-observations', 'cases': data['cases']}],
                           self.manifest, render.render_inventory(self.inventory))
        with self.assertRaisesRegex(ValueError, 'paired'):
            render.sound_observations({'cases': []})

        white, black, red = b'\xff\xff\xff', b'\0\0\0', b'\xff\0\0'
        pixels = white + black + white + red + white + black
        observed = render.drawing_observation((3, 2, pixels))
        self.assertEqual(observed['nonwhite_pixels'], 3)
        self.assertEqual(observed['nonwhite_bounds'], [0, 0, 3, 2])
        self.assertEqual(observed['rgb_sha256'], render.sha(pixels))
        self.assertFalse(pixel_pass(self.manifest['cases'][0], observed))
        blank = render.drawing_observation((3, 2, white * 6))
        self.assertEqual(blank['nonwhite_pixels'], 0)
        self.assertIsNone(blank['nonwhite_bounds'])

    def test_rendering_never_infers_a_refusal_from_an_arbitrary_error(self):
        row = render.fixtures(render.sound_observations(self.manifest), observations=True)[
            'annotations.display.sound-speaker-missing-ap']
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            def run(stdout=b'', stderr=b'', pixels=None):
                def stub(command, **kwargs):
                    kwargs['stdout'].write(stdout)
                    kwargs['stderr'].write(stderr)
                    if pixels is not None:
                        (directory / 'page.ppm').write_bytes(pixels)
                    return subprocess.CompletedProcess(command, 0 if pixels is not None else 1)
                return stub
            with patch.object(render.subprocess, 'run', side_effect=run(pixels=b'P6\n1 1\n255\n\xff\xff\xff')):
                observed = render.invoke(['stub'], row, directory, observations=True, engine='pdfium')
                self.assertEqual(observed['outcome'], 'observed')
                self.assertNotIn('pixel_comparison', observed)
                drawing = observed['observed']
                assert isinstance(drawing, dict)
                self.assertEqual(drawing['nonwhite_pixels'], 0)
            for engine, stdout, stderr, outcome in [
                ('pagura', b'', b'pagura: unsupported [annotation.appearance]: missing', 'unsupported'),
                ('pdfium', b'{"schema":1,"status":"unsupported","feature":"sound.icon"}', b'', 'unsupported'),
                ('pdfium', b'', b'unsupported sound', 'renderer-error'),
                ('pdfium', b'{"schema":true,"status":"unsupported","feature":"sound.icon"}', b'', 'renderer-error'),
                ('pagura', b'', b'could not open the document', 'renderer-error'),
                ('pdfium', b'', b'pagura: unsupported [annotation.appearance]: missing', 'renderer-error'),
                ('pagura', b'', b'pagura: unsupported [invalid tag]: missing', 'renderer-error'),
            ]:
                for observations in (False, True):
                    with self.subTest(engine=engine, outcome=outcome, observations=observations), patch.object(
                            render.subprocess, 'run', side_effect=run(stdout, stderr)):
                        result = render.invoke(['stub'], row, directory, observations=observations, engine=engine)
                        expected = 'adapter-error' if outcome == 'renderer-error' and not observations else outcome
                        self.assertEqual(result['outcome'], expected)
                        self.assertNotIn('pixel_comparison', result)
                        self.assertNotIn('observed', result)

    def test_redact_observations_keep_preview_separate_from_application(self):
        data = render.redact_observations(self.manifest)
        rows = render.fixtures(data, observations=True)
        self.assertEqual(len(rows), 18)
        for variant in ('rect', 'quads', 'border-color', 'interior', 'quad-interior',
                        'overlay', 'repeat', 'replacement-form'):
            for suffix in ('missing', 'stored'):
                key = 'annotations.display.redact-observe-' + variant + '-' + suffix
                self.assertEqual(rows[key]['dpi'], 72)
                self.assertNotIn('oracle', rows[key])
        self.assertIn('annotations.display.redact-observe-ordinary-content', rows)
        with self.assertRaisesRegex(ValueError, 'paired'):
            render.redact_observations({'cases': []})
        original = render.read_json
        def missing_pair(path):
            data = original(path)
            data['cases'] = [row for row in data['cases']
                             if row['id'] != 'annotation_display/redact-observe-repeat-stored']
            return data
        with patch.object(render, 'read_json', side_effect=missing_pair), self.assertRaisesRegex(ValueError, 'paired'):
            render.redact_observations(self.manifest)
        with self.assertRaises(ValueError):
            render.validate_manifest(data, self.inventory)


if __name__ == '__main__':
    unittest.main()
