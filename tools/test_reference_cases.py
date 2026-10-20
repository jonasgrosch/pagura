"""Reference binding tests; placeholder sources are not production PDF proofs.

No product executable or real fixture generator is invoked by these tests.
Conformance subprocesses are replaced by explicit byte-writing test doubles.
"""
import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import conformance
import fixtures
from benchkit import corpus, reference_cases as refs
from benchkit.oracles import reference_digest
from fixturegen.model import Case, RENDER


class ReferenceCaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for relative in refs.COMMON_SOURCES:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"test source: " + relative.encode())
        self.pdf = b"%PDF-test-source-not-an-independent-fixture"
        self.oracle = b"P6\n32 32\n255\n" + bytes([42]) * (32 * 32 * 3)
        self.case = {
            "id": "paint/image-direct-binary-halves", "kind": "pdf", "expected": "render",
            "path": "corpus/generated/paint/image-direct-binary-halves.pdf",
            "sha256": refs.sha(self.pdf), "pages": 1, "strict_valid": True, "repairs": [],
            "oracle": "corpus/generated/paint/image-direct-binary-halves.ppm",
            "oracle_sha256": refs.sha(self.oracle), "dpi": 72, "width": 32, "height": 32,
            "tolerance": 0, "reference_profile": refs.PROFILE,
        }
        self.write_inputs(self.case)
        self.case["render_references"] = refs.build_case_references(
            self.root, self.case, pdf_bytes=self.pdf, oracle_bytes=self.oracle)

    def write_inputs(self, case):
        for key, data in (("path", self.pdf), ("oracle", self.oracle)):
            path = self.root / case[key]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

    def settings(self, mode="exact", **updates):
        arguments = {"page": 0, "dpi": 72, "accuracy": mode, "entrypoint": "render_page",
                     "validation": "full" if mode == "exact" else "drawing-only",
                     "render_policy_sha256": refs.current_render_policy_sha256(self.root)}
        arguments.update(updates)
        return refs.invocation_settings(refs.sha(self.pdf), **arguments)

    def verify(self, actual=None, mode="exact", **overrides):
        arguments = {"settings": self.settings(mode), "source_bytes": self.pdf}
        arguments.update(overrides)
        return refs.verify_case_reference(self.root, self.case, mode,
            self.oracle if actual is None else actual, **arguments)

    def test_modes_bind_real_input_and_output_bytes(self):
        for mode, validation in (("exact", "full"), ("competitive", "drawing-only")):
            metadata, pin = refs.validate_case_reference(self.case, mode)
            self.assertEqual(pin, reference_digest(metadata))
            self.assertEqual(metadata["settings"]["validation"], validation)
            self.assertEqual(set(metadata["sources"]), set(refs.COMMON_SOURCES) | {self.case["path"]})
            result = self.verify(mode=mode)
            self.assertEqual(result["status"], "ok")
            self.assertEqual(result["actual_sha256"], refs.sha(self.oracle))
        wrong = self.oracle[:-1] + bytes([43])
        self.assertEqual(self.verify(wrong)["status"], "incorrect")

    def test_fractional_case_preserves_existing_mode_bounds(self):
        case = copy.deepcopy(self.case)
        case.pop("render_references")
        case["id"] = "paint/image-direct-rgb-fractional"
        case["path"] = "corpus/generated/paint/image-direct-rgb-fractional.pdf"
        case["oracle"] = "corpus/generated/paint/image-direct-rgb-fractional.ppm"
        case["competitive_tolerance"] = 1
        self.write_inputs(case)
        case["render_references"] = refs.build_case_references(
            self.root, case, pdf_bytes=self.pdf, oracle_bytes=self.oracle)
        self.assertEqual(refs.validate_case_reference(case, "exact")[0]["max_channel_error"], 0)
        self.assertEqual(refs.validate_case_reference(case, "competitive")[0]["max_channel_error"], 1)
        self.case = case
        self.assertEqual(self.verify(self.oracle[:-1] + bytes([43]), "competitive")["status"], "ok")
        self.assertEqual(self.verify(self.oracle[:-1] + bytes([44]), "competitive")["status"], "incorrect")

    def test_known_or_annotated_cases_cannot_silently_lose_references(self):
        for mutation in (lambda case: case.pop("render_references"),
                         lambda case: case.pop("reference_profile"),
                         lambda case: case.update(reference_profile="unknown")):
            case = copy.deepcopy(self.case)
            mutation(case)
            self.assertTrue(refs.requires_reference(case))
            with self.assertRaises(ValueError):
                refs.validate_case_reference(case, "exact")
        self.assertFalse(refs.requires_reference({"id": "other/fixture"}))
        self.assertTrue(refs.requires_reference({"id": "other/fixture", "render_references": {}}))

    def test_final_case_fields_and_source_set_must_agree(self):
        for key, value in (("id", []), ("dpi", True), ("width", 31), ("page", 1),
                           ("pages", 2), ("sha256", "0" * 64), ("oracle_sha256", "0" * 64),
                           ("strict_valid", False), ("repairs", ["repair"]), ("password", "")):
            with self.subTest(key=key):
                case = copy.deepcopy(self.case)
                case[key] = value
                with self.assertRaises(ValueError):
                    refs.validate_case_reference(case, "exact")
        case = copy.deepcopy(self.case)
        metadata = case["render_references"]["exact"]["metadata"]
        metadata["sources"]["unexpected.py"] = refs.sha(b"extra")
        case["render_references"]["exact"]["sha256"] = reference_digest(metadata)
        with self.assertRaisesRegex(ValueError, "source set"):
            refs.validate_case_reference(case, "exact")

    def test_pin_mutation_partial_modes_and_tolerance_escalation_fail(self):
        case = copy.deepcopy(self.case)
        case["render_references"]["exact"]["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "frozen pin"):
            refs.validate_case_reference(case, "exact")
        case = copy.deepcopy(self.case)
        del case["render_references"]["competitive"]
        with self.assertRaisesRegex(ValueError, "both mode"):
            refs.validate_case_reference(case, "exact")
        case = copy.deepcopy(self.case)
        case["tolerance"] = 1
        for record in case["render_references"].values():
            record["metadata"]["max_channel_error"] = 1
            record["sha256"] = reference_digest(record["metadata"])
        with self.assertRaisesRegex(ValueError, "cannot change"):
            refs.validate_case_reference(case, "exact")

    def test_unsupported_actual_invocations_fail(self):
        for update in ({"page": 1}, {"dpi": 144}, {"dpi": True}, {"profile": "unknown"},
                       {"accuracy": "approximate"}, {"validation": "drawing-only"},
                       {"entrypoint": "compile_page+render"}, {"render_policy_sha256": "bad"}):
            with self.subTest(update=update), self.assertRaises(ValueError):
                self.settings(**update)
        changed = self.settings()
        changed["source_pdf_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "actual settings"):
            self.verify(settings=changed)
        with self.assertRaisesRegex(ValueError, "actual PDF snapshot"):
            self.verify(source_bytes=b"other PDF")

    def test_source_policy_derivation_and_oracle_drift_fail(self):
        for relative in (refs.POLICY_PATH, refs.DERIVATION_PATH, self.case["path"],
                         "tools/fixturegen/pdf.py", "tools/fixturegen/image.py", self.case["oracle"]):
            with self.subTest(relative=relative):
                path = self.root / relative
                original = path.read_bytes()
                path.write_bytes(original + b"changed")
                with self.assertRaises(ValueError):
                    self.verify()
                path.write_bytes(original)

    def test_bounded_root_contained_source_loading(self):
        metadata, _ = refs.validate_case_reference(self.case, "exact")
        for relative in ("../escape", "/absolute", "docs/../escape", "docs//x", "docs\\x"):
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                refs.read_reference_file(self.root, relative)
        with patch.object(refs, "MAX_SOURCE_FILES", 1), self.assertRaises(ValueError):
            refs.load_reference_sources(self.root, metadata)
        with patch.object(refs, "MAX_SOURCE_BYTES", 8), self.assertRaises(ValueError):
            refs.load_reference_sources(self.root, metadata)
        with patch.object(refs, "MAX_TOTAL_SOURCE_BYTES", 8), self.assertRaises(ValueError):
            refs.load_reference_sources(self.root, metadata)
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside) / "policy.md"
            external.write_bytes((self.root / refs.POLICY_PATH).read_bytes())
            (self.root / refs.POLICY_PATH).unlink()
            (self.root / refs.POLICY_PATH).symlink_to(external)
            with self.assertRaisesRegex(ValueError, "escapes root"):
                refs.load_reference_sources(self.root, metadata)

    def test_generation_binds_only_after_extra_and_competitive_bounds(self):
        record = Case("image-direct-rgb-fractional", self.pdf, RENDER, ("paint-image-resampling",),
                      "binding test", oracle=self.oracle,
                      extra={"reference_profile": refs.PROFILE, "tolerance": 0})
        module = SimpleNamespace(AREA="paint", cases=lambda: [record])
        generated, manifests = self.root / "corpus/generated", self.root / "corpus/manifests"
        with patch.multiple(fixtures, ROOT=self.root, GENERATED=generated, MANIFESTS=manifests):
            files = fixtures.render_area(module)
        entry = json.loads(files[manifests / "paint.json"])["cases"][0]
        self.assertEqual(entry["competitive_tolerance"], 1)
        self.assertEqual(refs.validate_case_reference(entry, "competitive")[0]["max_channel_error"], 1)
        record.extra["sha256"] = "0" * 64
        with patch.multiple(fixtures, ROOT=self.root, GENERATED=generated, MANIFESTS=manifests):
            with self.assertRaisesRegex(ValueError, "PDF identity"):
                fixtures.render_area(module)

    def test_alternate_corpus_page_or_dpi_drops_reference_and_oracle(self):
        case = copy.deepcopy(self.case)
        case["pages"] = 2
        with patch.object(corpus, "ROOT", self.root), patch.object(corpus, "render_cases", return_value=[case]), \
                patch.object(corpus, "manifest_digest", return_value="frozen"):
            clones, _ = corpus.corpus_cases(None, None, dpis=(72, 144))
        for clone in clones:
            bound_view = clone["page"] == 0 and clone["dpi"] == 72
            for key in ("oracle", "oracle_sha256", "reference_profile", "render_references"):
                self.assertEqual(key in clone, bound_view, (key, clone["page"], clone["dpi"]))

    def test_conformance_uses_snapshot_and_explicit_actual_settings(self):
        calls, snapshots = [], []
        report = {"open": None, "pages": 1, "repairs": [], "information": [],
                  "strict": True, "failures": []}

        def invoke(*args, **kwargs):
            calls.append(args)
            if args[0] == "scan":
                snapshots.append(Path(args[-1]))
                self.assertNotEqual(snapshots[-1], self.root / self.case["path"])
                self.assertEqual(snapshots[-1].read_bytes(), self.pdf)
                return subprocess.CompletedProcess(args, 0, json.dumps(report).encode(), b"")
            self.assertEqual(args[0], "render")
            self.assertEqual(args[1:5], ("--accuracy", "exact" if len(calls) == 2 else "competitive", "--overprint", "auto"))
            self.assertEqual(args[-2:], ("72", "0"))
            self.assertEqual(Path(args[-4]), snapshots[0])
            self.assertEqual(Path(args[-4]).read_bytes(), self.pdf)
            Path(args[-3]).write_bytes(self.oracle)
            return subprocess.CompletedProcess(args, 0, b"", b"")

        with patch.object(conformance, "ROOT", self.root), patch.object(conformance, "pagura", side_effect=invoke):
            self.assertEqual(conformance.check(self.case), [])
        self.assertEqual(len(calls), 3)
        self.assertFalse(snapshots[0].exists())

    def test_conformance_refuses_missing_or_stale_metadata_before_invocation(self):
        case = copy.deepcopy(self.case)
        del case["render_references"]
        with patch.object(conformance, "ROOT", self.root), patch.object(conformance, "pagura") as invoke:
            self.assertIn("reference verification failed", conformance.check(case)[0])
            invoke.assert_not_called()
        (self.root / refs.DERIVATION_PATH).write_bytes(b"changed")
        with patch.object(conformance, "ROOT", self.root), patch.object(conformance, "pagura") as invoke:
            self.assertIn("source hash mismatch", conformance.check(self.case)[0])
            invoke.assert_not_called()


if __name__ == "__main__":
    unittest.main()
