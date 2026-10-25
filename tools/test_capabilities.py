"""Prevent source/tag inventories from becoming false runtime parity claims."""
import copy
import tempfile
import unittest
from pathlib import Path

import capabilities as cap


class InventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.original = cap.read_json(cap.ROOT / cap.LEDGER)

    def setUp(self):
        self.data = copy.deepcopy(self.original)

    def rejects(self, message):
        with self.assertRaisesRegex(ValueError, message):
            cap.validate(self.data, cap.ROOT)

    def test_committed_source_inventory_is_valid_but_has_no_runtime_verdicts(self):
        cap.validate(self.data, cap.ROOT)
        text = cap.render(self.data)
        self.assertIn("G00 source inventory is complete", text)
        self.assertIn("not executed feature tests", text)
        self.assertIn("All competitor family cells remain unprobed", text)

    def test_missing_baseline_is_not_inapplicability(self):
        del self.data["capabilities"][0]["comparisons"]["pdfium"]
        self.rejects("missing baseline comparison")

    def test_positive_and_inapplicable_claims_require_a_future_probe_contract(self):
        for result in ("supported", "unsupported", "inapplicable", "pass", None):
            with self.subTest(result=result):
                self.data["capabilities"][0]["comparisons"]["pdfium"] = result
                self.rejects("cannot assert runtime support or N/A")

    def test_engine_denominator_cannot_be_shrunk(self):
        self.data["engines"].remove("hayro")
        self.rejects("baseline engines differ")

    def test_duplicate_empty_and_unowned_rows_fail(self):
        self.data["capabilities"].append(copy.deepcopy(self.data["capabilities"][0]))
        self.rejects("duplicate capability")
        self.data["capabilities"] = []
        self.rejects("empty capability inventory")
        self.data = copy.deepcopy(self.original)
        self.data["capabilities"][0]["package"] = "R99"
        self.rejects("unknown work package")

    def test_invalid_requirements_fixtures_and_owners_fail(self):
        for field, expected in (("requirements", "unknown requirement"), ("fixtures", "unknown fixture"), ("owners", "unknown owner")):
            with self.subTest(field=field):
                self.data = copy.deepcopy(self.original)
                self.data["capabilities"][0][field] = ["does-not-exist"]
                self.rejects(expected)

    def test_changed_source_cannot_keep_its_old_audit(self):
        path = next(iter(self.data["source_files"]))
        self.data["source_files"][path] = "0" * 64
        self.rejects("source changed; review affected capabilities")

    def test_evidence_cannot_escape_its_hash_inventory(self):
        path = self.data["capabilities"][0]["sources"][0]
        del self.data["source_files"][path]
        self.rejects("unhashed evidence")

    def test_missing_or_relocated_tag_candidates_need_review(self):
        tag = next(iter(self.data["feature_tags"]))
        del self.data["feature_tags"][tag]
        self.rejects("feature-tag inventory drift")
        self.data = copy.deepcopy(self.original)
        self.data["feature_tags"][tag]["sources"] = ["wrong.rs"]
        self.rejects("source locations changed")

    def test_reviews_require_evidence_and_known_owners(self):
        tag = next(iter(self.data["feature_tags"]))
        self.data["feature_tags"][tag]["evidence"] = []
        self.rejects("missing adjudication evidence")
        self.data = copy.deepcopy(self.original)
        self.data["feature_tags"][tag]["capability"] = "missing.owner"
        self.rejects("unknown capability owner")

    def test_review_locator_cannot_silently_move(self):
        tag = next(iter(self.data["feature_tags"]))
        self.data["feature_tags"][tag]["evidence"][0]["needle"] = "nonexistent call site 738beef"
        self.rejects("stale evidence locator")

    def test_incomplete_manual_reviews_are_not_completion(self):
        tag = next(iter(self.data["feature_tags"]))
        self.data["feature_tags"][tag]["review"] = "unreviewed"
        self.rejects("unreviewed or invalid disposition")
        self.data = copy.deepcopy(self.original)
        self.data["capabilities"][0]["surfaces"]["host"] = "unreviewed"
        self.rejects("invalid surface state")

    def test_requirement_and_api_operations_cannot_be_unassigned(self):
        identity = self.data["capabilities"][0]["requirements"][0]
        for row in self.data["capabilities"]:
            row["requirements"] = [item for item in row["requirements"] if item != identity]
        self.rejects("unassigned requirements")
        self.data = copy.deepcopy(self.original)
        self.data["api_inventory"]["pdfium"]["operations"]["unknown.feature"] = "API"
        self.rejects("unassigned API operation")

    def test_api_inventory_retains_all_baselines(self):
        del self.data["api_inventory"]["quartz"]
        self.rejects("incomplete baseline API inventory")

    def test_malformed_schema_and_surfaces_fail(self):
        self.data["schema"] = True
        self.rejects("unsupported capability schema")
        self.data = copy.deepcopy(self.original)
        del self.data["capabilities"][0]["surfaces"]["host"]
        self.rejects("incomplete surface inventory")

    def test_unexpected_fields_cannot_smuggle_an_unchecked_verdict(self):
        self.data["capabilities"][0]["parity_verified"] = True
        self.rejects("unexpected capability row fields")


class SourceAndViewTests(unittest.TestCase):
    def test_duplicate_json_keys_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "duplicate.json"
            path.write_text('{"pdfium": "unprobed", "pdfium": "pass"}', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
                cap.read_json(path)

    def test_absolute_and_parent_evidence_paths_are_rejected(self):
        for path in ("../README.md", str(cap.ROOT / "README.md")):
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, "escaping evidence path"):
                cap.local_file(cap.ROOT, path)

    def test_scan_discovers_new_literals_without_claiming_reachability(self):
        with tempfile.TemporaryDirectory() as directory:
            # A parent folder named tests must not hide all product sources.
            root = Path(directory) / "tests"
            source = root / "crates/example/src"
            source.mkdir(parents=True)
            (source / "lib.rs").write_text('const A: Feature = Feature::new(\n "example.a"\n);\n// Feature::new("example.comment")\n', encoding="utf-8")
            (source / "tests.rs").write_text('Feature::new("test.only")', encoding="utf-8")
            self.assertEqual(cap.feature_tags(root), {
                "example.a": ["crates/example/src/lib.rs"],
                "example.comment": ["crates/example/src/lib.rs"],
            })
            with (source / "lib.rs").open("a", encoding="utf-8") as output:
                output.write('Feature::new("example.new")')
            self.assertIn("example.new", cap.feature_tags(root))

    def test_source_hashes_ignore_checkout_newlines_but_detect_content_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.rs"
            path.write_bytes(b"first\nsecond\n")
            digest = cap.source_hash(path)
            path.write_bytes(b"first\r\nsecond\r\n")
            self.assertEqual(cap.source_hash(path), digest)
            path.write_bytes(b"changed\nsecond\n")
            self.assertNotEqual(cap.source_hash(path), digest)

    def test_check_mode_detects_stale_or_missing_view_without_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "view.md"
            with self.assertRaisesRegex(ValueError, "stale capability view"):
                cap.update_view(path, "expected\n", check=True)
            self.assertFalse(path.exists())
            cap.update_view(path, "edited\n", check=False)
            with self.assertRaisesRegex(ValueError, "stale capability view"):
                cap.update_view(path, "expected\n", check=True)
            self.assertEqual(path.read_text(), "edited\n")
            cap.update_view(path, "expected\n", check=False)
            cap.update_view(path, "expected\n", check=True)


if __name__ == "__main__":
    unittest.main()
