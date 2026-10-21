"""Bounded protocol checks; no child launch, fixture generation or rendering."""
import unittest
from pathlib import Path

from f32_painted_interval_wasi import CELLS, diagnostic_plan, validate_build_context


class FixedProtocol(unittest.TestCase):
    def test_simd_has_fixed_inventory_and_balanced_336_outputs(self):
        plan = diagnostic_plan('simd')
        self.assertEqual(plan, diagnostic_plan('simd'))
        self.assertEqual((plan['seed'], plan['warmups'], plan['pairs'], plan['outputs']),
                         (20261111, 2, 12, 336))
        expected = tuple((name, mode) for name in (
            'opaque64', 'sparse64', 'image-only', 'f32-shading-dense',
            'f32-pattern-dense', 'default-knockout64') for mode in ('exact', 'competitive'))
        self.assertEqual(CELLS, expected)
        self.assertEqual(tuple((cell['case'], cell['mode']) for cell in plan['cells']), expected)
        self.assertEqual(plan['metric'], 'render_ns')
        for cell in plan['cells']:
            self.assertEqual(cell['operation'], 'full')
            self.assertEqual(len(cell['orders']), 14)
            self.assertCountEqual(cell['orders'][:2], [['baseline', 'candidate'], ['candidate', 'baseline']])
            self.assertEqual(cell['orders'][2:].count(['baseline', 'candidate']), 6)
            self.assertEqual(cell['orders'][2:].count(['candidate', 'baseline']), 6)

    def test_scalar_is_exactly_24_outputs_without_timing_metric(self):
        plan = diagnostic_plan('scalar')
        self.assertEqual((plan['seed'], plan['warmups'], plan['pairs'], plan['outputs']),
                         (20261111, 0, 1, 24))
        self.assertIsNone(plan['metric'])
        self.assertEqual(tuple((cell['case'], cell['mode']) for cell in plan['cells']), CELLS)
        for cell in plan['cells']:
            self.assertEqual(cell['operation'], 'full')
            self.assertEqual(cell['orders'], [['baseline', 'candidate']])

    def test_unknown_profile_is_rejected(self):
        with self.assertRaises(ValueError):
            diagnostic_plan('default')


class BuildBinding(unittest.TestCase):
    def context(self, feature):
        root = Path('/private/tmp/f32-wasi-unit/baseline')
        target = Path('/private/tmp/f32-wasi-unit/target-' + feature[0])
        flags = '-C target-feature=' + feature
        command = ['/usr/bin/env', '-u', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTFLAGS=' + flags,
                   '/private/tmp/f32-wasi-unit/cargo', 'build', '--locked', '--offline',
                   '--release', '-p', 'pagura-render', '--example', 'scene_index_profile',
                   '--target', 'wasm32-wasip1', '--target-dir', str(target), '-j', '1',
                   '--message-format=json-render-diagnostics']
        context = {'schema': 1, 'root': str(root), 'command': command,
                   'CARGO_TARGET_DIR': str(target), 'CARGO_BUILD_JOBS': '1',
                   'RUSTFLAGS': flags, 'CARGO_ENCODED_RUSTFLAGS': None}
        receipt = {'argv': ['/pinned/capture', 'build-label', *command], 'cwd': str(root)}
        module = target / 'wasm32-wasip1/release/examples/scene_index_profile.wasm'
        return context, receipt, root, module

    def test_actual_env_command_and_recorded_context_agree_for_both_features(self):
        for feature in ('+simd128', '-simd128'):
            context, receipt, root, module = self.context(feature)
            self.assertEqual(validate_build_context(context, receipt, root, feature, module),
                             Path(context['CARGO_TARGET_DIR']))

    def test_feature_label_cannot_replace_actual_scalar_argv(self):
        context, receipt, root, module = self.context('-simd128')
        receipt['argv'][5] = 'RUSTFLAGS=-C target-feature=+simd128'
        with self.assertRaises(ValueError):
            validate_build_context(context, receipt, root, '-simd128', module)

    def test_recorded_environment_and_module_target_are_required(self):
        context, receipt, root, module = self.context('-simd128')
        context['CARGO_ENCODED_RUSTFLAGS'] = '-C\x1ftarget-feature=+simd128'
        with self.assertRaises(ValueError):
            validate_build_context(context, receipt, root, '-simd128', module)
        context['CARGO_ENCODED_RUSTFLAGS'] = None
        with self.assertRaises(ValueError):
            validate_build_context(context, receipt, root, '-simd128',
                                   Path('/private/tmp/other/scene_index_profile.wasm'))


if __name__ == '__main__':
    unittest.main()
