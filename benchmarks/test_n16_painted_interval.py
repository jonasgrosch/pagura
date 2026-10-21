"""Focused matrix and literal-oracle checks; no renderer or subprocess needed."""
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import n16_output_u16 as previous
import n16_painted_interval as experiment
from evaluate_n16_output import cell_map, declared_cells, validate_guard
import n16_painted_interval_wasi as wasi
from evaluate_n16_painted_interval_wasi import expected_pixels


class FixedMatrix(unittest.TestCase):
    def test_legacy_default_inventory_and_roles_stay_unchanged(self):
        old = declared_cells(previous)
        self.assertEqual((len(previous.CASES), len(old)), (16, 46))
        self.assertEqual(sum(role == "target" for role in old.values()), 2)
        self.assertEqual(experiment.CELLS[:46], previous.CELLS)
        current = declared_cells(experiment, expected_case_count=18, expected_cell_count=52)
        self.assertEqual({cell: current[cell] for cell in previous.CELLS}, old)

    def test_new_matrix_is_exactly_six_protected_full_cells(self):
        declared = declared_cells(experiment, expected_case_count=18, expected_cell_count=52)
        self.assertEqual(set(experiment.CASES) - set(previous.CASES), {"opaque-empty", "opaque-distant"})
        self.assertEqual(set(declared) - set(previous.CELLS), {
            (name, mode, "full") for name in ("opaque-empty", "opaque-distant", "opaque-tiny")
            for mode in ("exact", "competitive")})
        self.assertEqual([cell for cell, role in declared.items() if role == "target"],
                         [("opaque64", "exact", "full"), ("opaque64", "competitive", "full")])
        self.assertEqual(sum(role == "protected" for role in declared.values()), 50)
        self.assertEqual(experiment.PHASE_PLAN, {
            "pilot": {"pairs": 5, "warmups": 5, "seed": 20261105},
            "confirmation": {"pairs": 200, "warmups": 5, "seed": 20261106}})

    def test_plan_template_matches_full_independent_inventory(self):
        plan = json.loads((Path(__file__).parent / "results/n16-painted-interval-plan.template.json").read_text())
        self.assertEqual(cell_map(plan["cells"], 52), declared_cells(experiment, 18, 52))
        self.assertEqual(plan["phases"], experiment.PHASE_PLAN)
        self.assertEqual(plan["expected_verified_outputs"], {"pilot": 1040, "confirmation": 21320})
        self.assertEqual(plan["status"], "template-not-frozen")
        self.assertIsNone(plan["identities_to_freeze"]["candidate"]["binary_sha256"])

    def test_wrong_defaults_counts_and_duplicates_fail_closed(self):
        with self.assertRaises(ValueError):
            declared_cells(experiment)
        with self.assertRaises(ValueError):
            declared_cells(experiment, 18, 51)
        with self.assertRaises(ValueError):
            declared_cells(previous, True, 46)
        changed = SimpleNamespace(CASES=experiment.CASES, CELLS=(*experiment.CELLS[:-1], experiment.CELLS[0]))
        with self.assertRaises(ValueError):
            declared_cells(changed, 18, 52)
        rows = [{"case": name, "mode": mode, "operation": operation, "role": role}
                for (name, mode, operation), role in declared_cells(experiment, 18, 52).items()]
        with self.assertRaises(ValueError):
            cell_map(rows)
        with self.assertRaises(ValueError):
            cell_map(rows[:-1], 52)
        with self.assertRaises(ValueError):
            cell_map([*rows[:-1], rows[0]], 52)

    def test_explicit_cold_target_does_not_retarget_default_full_cells(self):
        cold = SimpleNamespace(CASES=('many-operands', 'control'), CELLS=(
            ('many-operands', 'exact', 'cold'), ('many-operands', 'competitive', 'cold'),
            ('control', 'exact', 'cold'), ('control', 'competitive', 'cold')))
        actual = declared_cells(cold, 2, 4, target_case='many-operands', target_operation='cold')
        self.assertEqual([cell for cell, role in actual.items() if role == 'target'],
                         [('many-operands', 'exact', 'cold'), ('many-operands', 'competitive', 'cold')])
        self.assertEqual([cell for cell, role in declared_cells(previous).items() if role == 'target'],
                         [('opaque64', 'exact', 'full'), ('opaque64', 'competitive', 'full')])

    def test_wasi_has_ten_n16_cells_two_fallbacks_and_336_outputs(self):
        self.assertEqual(wasi.CELLS, tuple((name, mode) for name in
            ('opaque64', 'opaque-fill', 'opaque-empty', 'opaque-distant', 'opaque-tiny')
            for mode in ('exact', 'competitive')) + (('image-only', 'exact'), ('default-knockout64', 'exact')))
        self.assertEqual((wasi.PAIRS, wasi.WARMUPS, wasi.SEED, wasi.DPI), (12, 2, 20261107, 72))
        self.assertEqual(len(wasi.CELLS) * (wasi.PAIRS + wasi.WARMUPS) * 2, 336)
        plan = json.loads((Path(__file__).parent / 'results/n16-painted-interval-plan.template.json').read_text())
        self.assertEqual(plan['wasi_diagnostic']['expected_verified_outputs'], 336)
        self.assertIsNone(plan['identities_to_freeze']['baseline']['commit'])

    def test_current_multiple_target_seam_keeps_default_callers_unchanged(self):
        cases = ('primary', 'secondary', 'control')
        alternate = SimpleNamespace(CASES=cases, CELLS=tuple(
            (name, mode, 'cold') for name in cases for mode in ('exact', 'competitive')))
        declared = declared_cells(alternate, 3, 6, target_case='primary',
                                  target_operation='cold', target_cases=('primary', 'secondary'))
        self.assertEqual({cell for cell, role in declared.items() if role == 'target'},
                         {(name, mode, 'cold') for name in ('primary', 'secondary')
                          for mode in ('exact', 'competitive')})
        self.assertEqual({cell for cell, role in declared.items() if role == 'protected'},
                         {('control', mode, 'cold') for mode in ('exact', 'competitive')})
        self.assertEqual([cell for cell, role in declared_cells(previous).items() if role == 'target'],
                         [('opaque64', 'exact', 'full'), ('opaque64', 'competitive', 'full')])
        with self.assertRaises(ValueError):
            declared_cells(alternate, 3, 6, target_case='primary', target_operation='cold',
                           target_cases=('primary', 'primary'))


class StrictObserver(unittest.TestCase):
    def strict(self):
        return {'status': 'completed', 'observation_only': False, 'exit_code': 0, 'child_exit_code': 0,
                'events': [{'stage': 'preflight', 'matches': []}, {'stage': 'postflight', 'matches': []}]}

    def test_strict_is_explicit_and_rejects_missing_contaminated_or_cancelled_coverage(self):
        validate_guard(self.strict(), 'strict')
        historical = {'status': 'completed-observation-only', 'exit_code': 0}
        validate_guard(historical)
        with self.assertRaises(ValueError):
            validate_guard(historical, 'strict')
        with self.assertRaises(ValueError):
            validate_guard(self.strict())
        for changes in ({'exit_code': False}, {'child_exit_code': 1}, {'observation_only': True},
                        {'cancellation_requested_utc': '2026-01-01T00:00:00Z'},
                        {'events': [{'stage': 'preflight', 'matches': []}]},
                        {'events': [{'stage': 'preflight', 'matches': []},
                                    {'stage': 'postflight', 'matches': [{'pid': 123}]}]}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                validate_guard({**self.strict(), **changes}, 'strict')


class LiteralOracles(unittest.TestCase):
    def test_empty_has_no_paint_and_all_white_pixels(self):
        pdf, oracle = experiment.fixture("opaque-empty")
        self.assertIn(b"/MediaBox [0 0 512 512]", pdf)
        self.assertNotIn(b"re f", pdf)
        self.assertEqual(oracle, b"P6\n512 512\n255\n" + b"\xff" * (512 * 512 * 3))

    def test_distant_marks_cover_first_and_last_linear_pixel_in_each_chunk(self):
        pdf, oracle = experiment.fixture("opaque-distant")
        header = b"P6\n512 512\n255\n"
        self.assertTrue(oracle.startswith(header))
        pixels = oracle[len(header):]
        self.assertEqual(len(pixels), 512 * 512 * 3)
        black = []
        for offset in range(0, len(pixels), 3):
            pixel = pixels[offset:offset + 3]
            self.assertIn(pixel, (b"\0\0\0", b"\xff\xff\xff"))
            if pixel == b"\0\0\0":
                position = offset // 3
                black.append((position % 512, position // 512))
        self.assertEqual(len(black), 8)
        for cy in (0, 1):
            for cx in (0, 1):
                local = [256 * (y - 256 * cy) + x - 256 * cx for x, y in black
                         if x // 256 == cx and y // 256 == cy]
                self.assertEqual(local, [0, 65535])
        # Explicit PDF-space corners catch a missing top-down inversion even
        # though renderer execution is a separate coordinator-owned gate.
        self.assertIn(b"0 511 1 1 re f", pdf)
        self.assertIn(b"511 0 1 1 re f", pdf)
        self.assertEqual(pdf.count(b"1 1 re f"), 8)

    def test_existing_tiny_pdf_and_oracle_are_byte_identical(self):
        self.assertEqual(experiment.fixture("opaque-tiny"), previous.fixture("opaque-tiny"))

    def test_wasi_auditor_independent_empty_distant_and_tiny_formulas_agree(self):
        for name, size, black_count in (('opaque-empty', 512, 0), ('opaque-distant', 512, 8), ('opaque-tiny', 64, 64)):
            with self.subTest(case=name):
                actual_size, rgb, ppm, raw = expected_pixels(name)
                self.assertEqual(actual_size, size)
                self.assertEqual(ppm, experiment.fixture(name)[1])
                self.assertEqual(rgb.count(0), black_count * 3)
                header = f'RGBA8 {size} {size} premultiplied\n'.encode()
                self.assertTrue(raw.startswith(header))
                self.assertEqual(raw[len(header) + 3::4], b'\xff' * (size * size))
        _, tiny, _, _ = expected_pixels('opaque-tiny')
        self.assertEqual(tiny[(32 * 64 + 24) * 3:(32 * 64 + 25) * 3], b'\0\0\0')
        self.assertEqual(tiny[(31 * 64 + 24) * 3:(31 * 64 + 25) * 3], b'\xff\xff\xff')


class InterruptedArtifacts(unittest.TestCase):
    def test_interrupted_attempt_keeps_every_available_file_without_inventing_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = [root / name for name in ('stdout', 'stderr', 'metadata', 'raw')]
            for index, file in enumerate(files):
                file.write_bytes(bytes([index]))
            retained = {}
            def record(file):
                retained[file.name] = file.read_bytes()
            sample = {'status': 'cancelled', 'command': ['owned-node']}
            self.assertEqual(wasi.finish_attempt(sample, files, record), {})
            self.assertEqual(retained, {'stdout': b'\0', 'stderr': b'\1', 'metadata': b'\2', 'raw': b'\3'})
            self.assertEqual(sample['status'], 'cancelled')
            self.assertNotIn('process', sample)
            self.assertNotIn('diagnostic', sample)
            self.assertGreater(sample['finished_monotonic_ns'], 0)

    def test_retention_failure_does_not_hide_other_available_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            failed, good, absent = (root / name for name in ('failed', 'good', 'absent'))
            failed.write_bytes(b'bad')
            good.write_bytes(b'kept')
            retained = {}
            def record(file):
                if file == failed:
                    raise ValueError('fixed retention limit')
                retained[file.name] = file.read_bytes()
            sample = {'status': 'invalid-or-incomplete'}
            errors = wasi.finish_attempt(sample, (failed, good, absent), record)
            self.assertEqual(errors, {str(failed): 'ValueError: fixed retention limit'})
            self.assertEqual(sample['artifact_errors'], errors)
            self.assertEqual(retained, {'good': b'kept'})


if __name__ == "__main__":
    unittest.main()
