"""Unit tests for the corpus, scoreboard and conformance tooling."""
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

import conformance
import scoreboard
import zipcorpus


def ppm(width, height, pixels):
    return b"P6\n%d %d\n255\n" % (width, height) + bytes(pixels)


class ConformanceTests(unittest.TestCase):
    def test_images_compare_exactly_or_within_tolerance(self):
        base = ppm(1, 1, [10, 20, 30])
        self.assertIsNone(conformance.compare_images(base, base, 0))
        off_by_one = ppm(1, 1, [11, 20, 30])
        self.assertIn("up to 1", conformance.compare_images(off_by_one, base, 0))
        self.assertIsNone(conformance.compare_images(off_by_one, base, 1))
        self.assertIn("header", conformance.compare_images(ppm(2, 1, [0] * 6), base, 9))
        # Pixel bytes that are whitespace values must not be mistaken for separators.
        whitespace = ppm(1, 1, [9, 10, 32])
        self.assertIsNone(conformance.compare_images(whitespace, whitespace, 0))
        self.assertIn("up to 1", conformance.compare_images(ppm(1, 1, [9, 11, 32]), whitespace, 0))
        self.assertIn("payload", conformance.compare_images(ppm(1, 1, [9, 10]), ppm(1, 1, [9, 10]), 0))


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
        self.assertIn("| Timeouts | 1 |", summary)

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


if __name__ == "__main__":
    unittest.main()
