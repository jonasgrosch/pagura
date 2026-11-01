"""Unit tests for the corpus, scoreboard and conformance tooling."""
from copy import deepcopy
import hashlib
import io
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import conformance
import scoreboard
import zipcorpus


def ppm(width, height, pixels):
    return b"P6\n%d %d\n255\n" % (width, height) + bytes(pixels)


def mismatch(*arguments):
    problem = conformance.compare_images(*arguments)
    assert problem is not None, "images unexpectedly agree"
    return problem


class ConformanceTests(unittest.TestCase):
    def test_images_compare_exactly_or_within_tolerance(self):
        base = ppm(1, 1, [10, 20, 30])
        self.assertIsNone(conformance.compare_images(base, base, 0))
        off_by_one = ppm(1, 1, [11, 20, 30])
        self.assertIn("up to 1", mismatch(off_by_one, base, 0))
        self.assertIsNone(conformance.compare_images(off_by_one, base, 1))
        self.assertIn("header", mismatch(ppm(2, 1, [0] * 6), base, 9))
        # Pixel bytes that are whitespace values must not be mistaken for separators.
        whitespace = ppm(1, 1, [9, 10, 32])
        self.assertIsNone(conformance.compare_images(whitespace, whitespace, 0))
        self.assertIn("up to 1", mismatch(ppm(1, 1, [9, 11, 32]), whitespace, 0))
        self.assertIn("payload", mismatch(ppm(1, 1, [9, 10]), ppm(1, 1, [9, 10]), 0))


class ScoreboardTests(unittest.TestCase):
    def test_summary_counts_outcomes_and_tags(self):
        def scanned(strict, pages, rendered, failures=(), repairs=()):
            return {"outcome": "scanned", "strict": strict, "open": None, "pages": pages, "rendered": rendered,
                    "repairs": list(repairs), "failures": [{"page": 0, "error": f} for f in failures]}
        files = [
            {"result": scanned(True, 2, 2)},
            {"result": scanned(False, 1, 0, [{"kind": "unsupported", "feature": "operator.BT"}], ["xref.rebuilt"])},
            {"result": {"outcome": "scanned", "strict": False, "open": {"kind": "syntax", "feature": None}, "pages": None}},
            # A catalog that opens over a broken page tree (`/Kids 42`) is not opened.
            {"result": {"outcome": "scanned", "strict": False, "open": None, "pages": None, "rendered": 0, "repairs": [],
                        "failures": [{"page": None, "error": {"kind": "syntax", "feature": None}}]}},
            {"result": {"outcome": "not-pdf"}},
            {"result": {"outcome": "timeout"}},
        ]
        report = {"files": files, "created_at": "t", "commit": "c", "binary_sha256": "0" * 64,
                  "corpus_sha256": "1" * 64, "dpi": 72, "max_pages": 0}
        summary = scoreboard.summarize("unit", report)
        self.assertIn("| PDF files opened, pages enumerated | 2 / 5 (40.0%) |", summary)
        self.assertIn("| Opened, but the page tree failed | 1 |", summary)
        self.assertIn("| Page-tree failure | Count |\n|---|---:|\n| `syntax` | 1 |", summary)
        self.assertIn("| Pages rendered | 2 / 3 (66.7%) |", summary)
        self.assertIn("| `operator.BT` | 1 |", summary)
        self.assertIn("| `xref.rebuilt` | 1 |", summary)
        self.assertIn("| Timeouts (wall-clock budget) | 1 |", summary)

    @unittest.skipUnless(scoreboard.CPU_LIMITS, "CPU budgets need RLIMIT_CPU")
    def test_cpu_budget_turns_a_spinning_scan_into_a_timeout(self):
        with tempfile.TemporaryDirectory() as directory:
            spinner = Path(directory) / "spin"
            spinner.write_text("#!/bin/sh\nwhile :; do :; done\n")
            spinner.chmod(0o755)
            pdf = Path(directory) / "x.pdf"
            pdf.write_bytes(b"%PDF-1.7\n")
            arguments = type("Arguments", (), {"dpi": 72, "max_pages": 0, "timeout": 1})()
            original, scoreboard.BINARY = scoreboard.BINARY, spinner
            try:
                self.assertEqual(scoreboard.scan(pdf, arguments), {"outcome": "timeout", "budget": "cpu"})
            finally:
                scoreboard.BINARY = original

    def test_files_without_a_header_are_not_scanned(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "x.pdf"
            path.write_bytes(b"<html>" + b" " * 2000 + b"%PDF-1.7")
            self.assertFalse(scoreboard.looks_like_pdf(path))
            path.write_bytes(b"junk\n%PDF-1.7\n")
            self.assertTrue(scoreboard.looks_like_pdf(path))


class ZipCorpusTests(unittest.TestCase):
    def archive(self, names):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            for name in names:
                archive.writestr(name, b"%PDF-1.7\n")
        buffer.seek(0)
        return zipfile.ZipFile(buffer)

    def test_only_expected_member_names_are_extracted_and_flattened(self):
        corpus = zipcorpus.CORPORA["realworld"]
        names = ["0000001.pdf", "../escape.pdf", "nested/0000002.pdf", "/abs/0000003.pdf", "0000004.txt"]
        accepted = [name for _, name in zipcorpus.members(corpus, self.archive(names))]
        self.assertEqual(accepted, ["0000001.pdf"])
        adversarial = zipcorpus.CORPORA["adversarial"]
        good = "corpora-pdf/evalFour/02/94/" + "a" * 64
        accepted = [name for _, name in zipcorpus.members(adversarial, self.archive([good, good + "/../x", "corpora-pdf/x"]))]
        self.assertEqual(accepted, ["a" * 64 + ".pdf"])

    def test_extraction_enforces_the_size_cap(self):
        archive = self.archive(["0000001.pdf"])
        info = archive.infolist()[0]
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "0000001.pdf"
            original = zipcorpus.MAX_MEMBER_BYTES
            zipcorpus.MAX_MEMBER_BYTES = 4
            try:
                with self.assertRaises(SystemExit):
                    zipcorpus.extract(archive, info, target)
            finally:
                zipcorpus.MAX_MEMBER_BYTES = original
            self.assertFalse(target.exists())


PPM = b'P6\n1 1\n255\n\xff\x00\x00'
ERROR = {'kind': 'limit', 'message': 'image sample budget exceeded', 'feature': None, 'repair': None}


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.case = {'expected': 'open', 'path': 'case.pdf', 'pages': 2, 'dpi': 72, 'page_outcomes': [
            {'page': 0, 'expected': 'render', 'oracle': 'page.ppm', 'oracle_sha256': hashlib.sha256(PPM).hexdigest(),
             'width': 1, 'height': 1, 'tolerance': 0},
            {'page': 1, 'expected': 'refused', 'error': deepcopy(ERROR)}]}
        self.report = {'pages': 2, 'rendered': 1, 'failures': [{'page': 1, 'error': deepcopy(ERROR)}]}

    def test_complete_mixed_contract(self):
        self.assertEqual(conformance.page_report_problems(self.case, self.report), [])

    def test_all_invalid_failure_coverage(self):
        variants = [[], self.report['failures'] * 2]
        variants += [[{'page': page, 'error': deepcopy(ERROR)}] for page in (0, 2, -1, None, True)]
        variants += [self.report['failures'] + [{'page': 0, 'error': ERROR}]]
        for failures in variants:
            with self.subTest(failures=failures):
                report = deepcopy(self.report)
                report['failures'] = failures
                self.assertTrue(conformance.page_report_problems(self.case, report))

    def test_each_error_field_is_exact(self):
        for field in ERROR:
            report = deepcopy(self.report)
            report['failures'][0]['error'][field] = 'other'
            with self.subTest(field=field):
                self.assertTrue(conformance.page_report_problems(self.case, report))

    def test_rendered_count_and_actual_page_count(self):
        for key, value in [('rendered', 0), ('rendered', 2), ('rendered', True), ('pages', 1), ('pages', True)]:
            report = deepcopy(self.report)
            report[key] = value
            self.assertTrue(conformance.page_report_problems(self.case, report))

    def test_expected_indices_complete_and_integer(self):
        for rows in (self.case['page_outcomes'][:1], self.case['page_outcomes'] * 2):
            case = deepcopy(self.case)
            case['page_outcomes'] = rows
            with self.assertRaises(AssertionError):
                conformance.page_outcomes(case)
        for page in (0, 2, -1, None, True):
            case = deepcopy(self.case)
            case['page_outcomes'][1]['page'] = page
            with self.assertRaises(AssertionError):
                conformance.page_outcomes(case)

    def test_render_document_cannot_hide_refusal(self):
        case = deepcopy(self.case)
        case['expected'] = 'render'
        with self.assertRaises(AssertionError):
            conformance.page_outcomes(case)

    def call(self, case=None, *, bad_pixels=False, negative_exit=1, partial=False, positive_exit=0):
        case = case or self.case
        calls = []
        def fake(*args):
            self.assertEqual(args[0], 'render')
            self.assertEqual(args[-2], '72')
            page = int(args[-1])
            calls.append(page)
            output = Path(args[-3])
            row = next(row for row in case['page_outcomes'] if row['page'] == page)
            if row['expected'] == 'render':
                if positive_exit == 0:
                    output.write_bytes(PPM[:-1] + b'\x01' if bad_pixels else PPM)
                return subprocess.CompletedProcess(args, positive_exit, b'', b'failed')
            if partial:
                output.write_bytes(b'')
            return subprocess.CompletedProcess(args, negative_exit, b'', b'image sample budget exceeded (object 7 0)')
        report = deepcopy(self.report)
        refused = next(row['page'] for row in case['page_outcomes'] if row['expected'] == 'refused')
        report['failures'][0]['page'] = refused
        with tempfile.TemporaryDirectory() as directory, patch.object(conformance, 'ROOT', Path(directory)), patch.object(conformance, 'pagura', fake):
            (Path(directory) / 'page.ppm').write_bytes(PPM)
            result = conformance.check_page_outcomes(case, report)
        self.assertEqual(calls, [0, 1])
        return result

    def test_full_pixels_and_explicit_page_calls(self):
        self.assertEqual(self.call(), [])
        self.assertTrue(self.call(bad_pixels=True))
        self.assertTrue(self.call(positive_exit=1))

    def test_negative_exit_and_even_empty_partial_file_fail(self):
        self.assertTrue(self.call(negative_exit=0))
        self.assertTrue(self.call(negative_exit=2))
        self.assertTrue(self.call(partial=True))

    def test_reverse_order_keeps_later_positive_page(self):
        case = deepcopy(self.case)
        case['page_outcomes'][0]['page'] = 1
        case['page_outcomes'][1]['page'] = 0
        self.assertEqual(self.call(case), [])

    def test_oracle_identity_checked_before_render(self):
        for key, value in [('oracle_sha256', '0' * 64), ('width', 2), ('height', 2)]:
            case = deepcopy(self.case)
            case['page_outcomes'][0][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                self.call(case)



if __name__ == "__main__":
    unittest.main()
