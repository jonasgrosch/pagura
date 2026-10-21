"""Component checks for the independent auditor; no renderer or producer import."""
import unittest

from audit_f32_painted_interval_wasi import CELLS, decode, expected, p95, schedule


class IndependentEvidence(unittest.TestCase):
    def test_lattice_boundary_pixels_and_opacity(self):
        for name, level in [('opaque64', 0), ('sparse64', 191), ('default-knockout64', 191)]:
            side, rgb, ppm, raw = expected(name)
            self.assertEqual(side, 2048)
            self.assertEqual(len(rgb), side * side * 3)
            self.assertTrue(ppm.startswith(b'P6\n2048 2048\n255\n'))
            header = b'RGBA8 2048 2048 premultiplied\n'
            self.assertTrue(raw.startswith(header))
            pixels = raw[len(header):]
            self.assertEqual(pixels[3::4], bytes([255]) * (side * side))
            for x, y, value in [(0, 233, 255), (0, 234, level), (2047, 239, level),
                                (2047, 240, 255), (0, 2026, level), (2047, 2031, level), (0, 2032, 255)]:
                offset = (y * side + x) * 4
                self.assertEqual(pixels[offset:offset + 4], bytes([value, value, value, 255]))

    def test_dense_dynamic_sources_have_literal_uniform_oracles(self):
        for name in ('image-only', 'f32-shading-dense', 'f32-pattern-dense'):
            side, rgb, ppm, raw = expected(name)
            self.assertEqual((side, rgb), (512, bytes([191]) * (512 * 512 * 3)))
            self.assertEqual(ppm, b'P6\n512 512\n255\n' + rgb)
            self.assertEqual(raw, b'RGBA8 512 512 premultiplied\n' + bytes([191, 191, 191, 255]) * (512 * 512))

    def test_independent_schedule_preserves_counts_and_orders(self):
        simd, scalar = schedule('simd'), schedule('scalar')
        self.assertEqual(tuple((cell['case'], cell['mode']) for cell in simd), CELLS)
        self.assertEqual(sum(len(cell['orders']) * 2 for cell in simd), 336)
        self.assertEqual(sum(len(cell['orders']) * 2 for cell in scalar), 24)
        for cell in simd:
            self.assertEqual(cell['orders'][2:].count(['baseline', 'candidate']), 6)
            self.assertEqual(cell['orders'][2:].count(['candidate', 'baseline']), 6)
        for cell in scalar:
            self.assertEqual(cell['orders'], [['baseline', 'candidate']])
        with self.assertRaises(ValueError):
            schedule('other')

    def test_ambiguous_metadata_is_rejected_and_p95_is_linear(self):
        for source in (b'{"a":1,"a":2}', b'{"a":NaN}', b'{"a":Infinity}'):
            with self.assertRaises(ValueError):
                decode(source)
        self.assertAlmostEqual(p95(list(range(1, 13))), 11.45)


if __name__ == '__main__':
    unittest.main()
