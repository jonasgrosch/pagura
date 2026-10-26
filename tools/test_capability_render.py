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


if __name__ == '__main__':
    unittest.main()
