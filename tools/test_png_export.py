"""Hosted CLI PNG integration, decoded by the existing independent Python oracle.

No product encoder/decoder computes expected pixels. The binary is built by the
ordinary check workflow before unittest discovery; a missing binary is a failure.
"""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from benchkit.images import ppm
from fixturegen.codec import decode_png

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')) / 'debug' / ('pagura.exe' if os.name == 'nt' else 'pagura')
FIXTURES = ROOT / 'corpus/generated/classic'


class PngExportTests(unittest.TestCase):
    def invoke(self, *args, status=0):
        result = subprocess.run([str(BINARY), *map(str, args)], capture_output=True, timeout=30, cwd=ROOT)
        self.assertEqual(result.returncode, status, result.stderr.decode(errors='replace'))
        return result

    def test_opaque_pixels_match_independent_ppm_oracles(self):
        with tempfile.TemporaryDirectory() as temporary:
            for name in ['blank', 'rectangle', 'graphics-state', 'negative-rectangle', 'alpha-unsupported']:
                output = Path(temporary) / (name + '.png')
                self.invoke('render', '--strict', '--format', 'png', FIXTURES / (name + '.pdf'), output)
                width, height, channels, pixels, _ = decode_png(output.read_bytes())
                w, h, expected = ppm((FIXTURES / (name + '.ppm')).read_bytes())
                self.assertEqual((width, height, channels), (w, h, 4))
                self.assertEqual(pixels[3::4], bytes([255]) * (w * h))
                for channel in range(3):
                    self.assertEqual(pixels[channel::4], expected[channel::3], name)

    def test_transparent_output_retains_straight_color_and_partial_alpha(self):
        with tempfile.TemporaryDirectory() as temporary:
            for name in ['blank', 'rectangle', 'alpha-unsupported']:
                output = Path(temporary) / (name + '.png')
                self.invoke('render', '--format', 'png', '--transparent', FIXTURES / (name + '.pdf'), output)
                w, h, channels, pixels, _ = decode_png(output.read_bytes())
                self.assertEqual((w, h, channels), (64, 64, 4))
                expected = bytearray()
                for y in range(h):
                    for x in range(w):
                        if name == 'alpha-unsupported':
                            expected.extend([0, 0, 0, 128])
                        elif name == 'rectangle' and 8 <= x < 56 and 24 <= y < 56:
                            expected.extend([255, 0, 0, 255])
                        else:
                            expected.extend([0, 0, 0, 0])
                self.assertEqual(pixels, bytes(expected), name)

    def test_page_selection_dpi_timing_schema_and_existing_destination(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            result = self.invoke('convert', '--format', 'png', '--timings', FIXTURES / 'two-pages.pdf', directory, 144, 1, 1)
            report = json.loads(result.stdout)
            self.assertEqual([page['page'] for page in report['pages']], [1])
            self.assertEqual(sorted(path.name for path in directory.iterdir()), ['page-0002.png'])
            output = directory / 'page-0002.png'
            w, h, channels, pixels, _ = decode_png(output.read_bytes())
            self.assertEqual((w, h, channels), (128, 128, 4))
            expected = bytes(component for y in range(h) for x in range(w)
                             for component in ([0, 0, 255, 255] if x >= 64 and y < 64 else [255, 255, 255, 255]))
            self.assertEqual(pixels, expected)
            original = output.read_bytes()
            self.invoke('render', '--format', 'png', FIXTURES / 'blank.pdf', output, status=1)
            self.assertEqual(output.read_bytes(), original, 'existing file is never overwritten')
            repeated = directory / 'repeat.png'
            self.invoke('render', '--format', 'png', FIXTURES / 'two-pages.pdf', repeated, 144, 1)
            self.assertEqual(repeated.read_bytes(), original, 'encoding is deterministic across render/convert')

    def test_refusals_create_no_output_and_do_not_guess_a_format(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'output.png'
            self.invoke('render', '--format', 'png', FIXTURES / 'pattern-unsupported.pdf', output, status=1)
            self.assertFalse(output.exists())
            self.invoke('render', '--format', 'png', FIXTURES / 'blank.pdf', output, 72, 99, status=1)
            self.assertFalse(output.exists())
            self.invoke('render', '--transparent', FIXTURES / 'blank.pdf', output, status=2)
            self.assertFalse(output.exists())
            self.invoke('render', FIXTURES / 'rectangle.pdf', output)
            self.assertEqual(output.read_bytes(), (FIXTURES / 'rectangle.ppm').read_bytes(), 'default PPM remains byte-identical even with a .png path')


if __name__ == '__main__':
    unittest.main()
