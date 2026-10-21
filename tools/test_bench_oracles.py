"""Hand-authored reference bytes; no renderer, fixture generator, or subprocess."""
import copy
import hashlib
import unittest

from benchkit.oracles import reference_digest, verify_reference


def sha(data):
    return hashlib.sha256(data).hexdigest()


def image(pixels=b'\x00\x80\xff', width=1, height=1):
    return f'P6\n{width} {height}\n255\n'.encode() + pixels


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        # Binding examples only: these are not production PDFs or mathematical proofs.
        self.sources = {'input.pdf': b'test PDF identity', 'policy.md': b'test rendering policy',
                        'derivation.md': b'hand-authored RGB test vector'}
        self.oracle = image()
        self.metadata = {
            'schema': 1, 'case_id': 'content/reference-contract-test',
            'settings': {
                'source_pdf_sha256': sha(self.sources['input.pdf']),
                'render_policy_sha256': sha(self.sources['policy.md']),
                'page': 0, 'dpi': 72, 'background': 'white', 'output': 'RGB8 PPM',
                'box': 'MediaBox=CropBox', 'annotations': True,
                'accuracy': 'exact', 'entrypoint': 'render_page', 'validation': 'full',
            },
            'dimensions': [1, 1],
            'reference': {'model': 'per-mark-coverage-rgb8', 'evidence_class': 'exact_rational',
                          'rounding': 'nearest-half-up', 'derivation': 'derivation.md'},
            'oracle_sha256': sha(self.oracle),
            'sources': {path: sha(data) for path, data in self.sources.items()},
            'max_channel_error': 0,
        }
        self.pin = reference_digest(self.metadata)

    def verify(self, actual=None, **overrides):
        arguments = {'actual_ppm': self.oracle if actual is None else actual,
                     'oracle_ppm': self.oracle, 'metadata': self.metadata,
                     'pinned_digest': self.pin, 'settings': self.metadata['settings'],
                     'sources': self.sources}
        arguments.update(overrides)
        return verify_reference(**arguments)

    def test_exact_bytes_and_actual_hashes(self):
        result = self.verify()
        self.assertEqual(result['status'], 'ok')
        self.assertEqual(result['actual_sha256'], sha(self.oracle))
        self.assertEqual(result['actual_pixels_sha256'], sha(b'\x00\x80\xff'))
        self.assertEqual(result['reference_digest'], self.pin)
        self.assertEqual(result['oracle_difference']['max_channel_error'], 0)

    def test_canonical_digest_ignores_mapping_order(self):
        reordered = dict(reversed(list(self.metadata.items())))
        reordered['settings'] = dict(reversed(list(self.metadata['settings'].items())))
        self.assertEqual(reference_digest(reordered), self.pin)

    def test_zero_and_one_bounds_apply_to_every_channel(self):
        changed = image(b'\x01\x7f\xfe')
        self.assertEqual(self.verify(changed)['status'], 'incorrect')
        self.metadata['max_channel_error'] = 1
        self.pin = reference_digest(self.metadata)
        result = self.verify(changed)
        self.assertEqual(result['status'], 'ok')
        self.assertEqual(result['oracle_difference']['different_channels'], 3)
        self.assertEqual(result['oracle_difference']['max_channel_error'], 1)
        self.assertEqual(self.verify(image(b'\x02\x80\xff'))['status'], 'incorrect')

    def test_tolerance_escalation_cannot_reuse_pin(self):
        self.metadata['max_channel_error'] = 1
        with self.assertRaisesRegex(ValueError, 'frozen pin'):
            self.verify(image(b'\x01\x80\xff'))

    def test_missing_or_invalid_pin_and_settings_fail_closed(self):
        for pin in (None, '', 'A' * 64, 'g' * 64, 'a' * 64):
            with self.subTest(pin=pin), self.assertRaises(ValueError):
                self.verify(pinned_digest=pin)
        for settings in ({}, None, {**self.metadata['settings'], 'dpi': float('nan')},
                         {**self.metadata['settings'], 'annotations': 1},
                         {**self.metadata['settings'], 'unexpected': True}):
            with self.subTest(settings=settings), self.assertRaises(ValueError):
                self.verify(settings=settings)

    def test_dimension_mismatch_is_incorrect_but_bad_oracle_metadata_is_invalid(self):
        result = self.verify(image(b'\x00\x80\xff' * 2, width=2))
        self.assertEqual(result['status'], 'incorrect')
        self.assertEqual(result['oracle_difference']['status'], 'dimension-mismatch')
        self.metadata['dimensions'] = [2, 1]
        self.pin = reference_digest(self.metadata)
        with self.assertRaisesRegex(ValueError, 'oracle dimensions'):
            self.verify()

    def test_same_pixels_with_different_header_do_not_replace_oracle_artifact(self):
        different_header = b'P6\n# comment\n1 1\n255\n\x00\x80\xff'
        self.assertEqual(self.verify(different_header)['status'], 'ok')
        with self.assertRaisesRegex(ValueError, 'artifact hash'):
            self.verify(oracle_ppm=different_header)

    def test_changed_settings_and_policy_identities_are_rejected(self):
        for key, value in (('dpi', 144), ('page', 1), ('accuracy', 'competitive'),
                           ('entrypoint', 'compile_page+render'),
                           ('source_pdf_sha256', 'a' * 64), ('render_policy_sha256', 'b' * 64)):
            settings = {**self.metadata['settings'], key: value}
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'actual settings'):
                self.verify(settings=settings)
        with self.assertRaises(ValueError):
            self.verify(settings={**self.metadata['settings'], 'page': False})

    def test_validation_scope_is_not_interchangeable(self):
        self.metadata['settings'].update(accuracy='competitive', validation='drawing-only')
        self.pin = reference_digest(self.metadata)
        self.assertEqual(self.verify()['status'], 'ok')
        with self.assertRaisesRegex(ValueError, 'actual settings'):
            self.verify(settings={**self.metadata['settings'], 'validation': 'full'})
        self.metadata['settings']['entrypoint'] = 'compile_page+render'
        with self.assertRaisesRegex(ValueError, 'drawing-only'):
            reference_digest(self.metadata)

    def test_all_bound_sources_must_be_present_unchanged_and_without_extras(self):
        for sources in ({}, {**self.sources, 'extra': b'data'},
                        {**self.sources, 'derivation.md': b'changed'},
                        {**self.sources, 'policy.md': bytearray(self.sources['policy.md'])}):
            with self.subTest(sources=sources), self.assertRaises(ValueError):
                self.verify(sources=sources)

    def test_source_and_policy_hashes_must_bind_supplied_data(self):
        for key in ('source_pdf_sha256', 'render_policy_sha256'):
            metadata = copy.deepcopy(self.metadata)
            metadata['settings'][key] = 'a' * 64
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'bound source'):
                reference_digest(metadata)

    def test_noncanonical_source_paths_are_rejected(self):
        for path in ('', '/absolute', '../escape', './file', 'a/../b', 'a//b', 'a/',
                     'a\\b', 'a\0b', 12):
            metadata = copy.deepcopy(self.metadata)
            metadata['sources'][path] = 'a' * 64
            with self.subTest(path=path), self.assertRaises(ValueError):
                reference_digest(metadata)

    def test_unsupported_reference_claims_are_rejected(self):
        for key, value in (('model', 'continuous-scene-rgb8'),
                           ('evidence_class', 'rigorous_enclosure'),
                           ('evidence_class', 'independent_numerical'),
                           ('rounding', 'unknown'), ('derivation', 'missing.md')):
            metadata = copy.deepcopy(self.metadata)
            metadata['reference'][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                reference_digest(metadata)

    def test_invalid_schema_dimensions_bounds_digests_and_status_fields(self):
        for key, value in (('schema', True), ('schema', 2), ('case_id', ''),
                           ('dimensions', [True, 1]), ('dimensions', [0, 1]),
                           ('dimensions', [16_000_001, 1]), ('dimensions', [1.0, 1]),
                           ('max_channel_error', True), ('max_channel_error', -1),
                           ('max_channel_error', 2), ('max_channel_error', float('nan')),
                           ('oracle_sha256', 'A' * 64), ('oracle_sha256', 'g' * 64),
                           ('status', 'ok')):
            metadata = copy.deepcopy(self.metadata)
            metadata[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                reference_digest(metadata)
        for key in self.metadata:
            metadata = copy.deepcopy(self.metadata)
            del metadata[key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                reference_digest(metadata)

    def test_rgb8_only_and_no_partial_or_mutable_outputs(self):
        for actual in (b'P6\n1 1\n255\n\0\0', image() + b'extra',
                       b'RGBA8 1 1 straight\n\0\x80\xff\xff', bytearray(image())):
            with self.subTest(actual=actual), self.assertRaises(ValueError):
                self.verify(actual)
        malformed = b'P6\n1 1\n255\n\0\0'
        self.metadata['oracle_sha256'] = sha(malformed)
        self.pin = reference_digest(self.metadata)
        with self.assertRaises(ValueError):
            self.verify(oracle_ppm=malformed)


if __name__ == '__main__':
    unittest.main()
