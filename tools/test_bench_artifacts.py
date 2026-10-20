"""Retained bytes and invocation binding controls; no real PDF engines needed."""
import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchkit.artifacts import ArtifactStore, read_bounded_regular, hash_regular_file, digest, verify_artifact_binding
from benchkit.config import ROOT, sha
from benchkit.corpus import render_cases
from benchkit.reference_cases import invocation_settings, current_render_policy_sha256
from benchkit.images import normalize
from benchkit import measurement


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.engine = self.directory / "engine"
        self.engine.write_bytes(b"#!/bin/sh\nexit 0\n")
        self.engine.chmod(0o700)
        self.source = self.directory / "input.pdf"
        self.source.write_bytes(b"frozen PDF input bytes")
        self.case = {"id": "artifact-test", "path": str(self.source),
                     "sha256": sha(self.source.read_bytes()), "dpi": 72, "page": 0}
        identity = {"status": "available", "command": [str(self.engine)],
                    "artifacts": {str(self.engine): sha(self.engine.read_bytes())}}
        self.report = {"operation": "render-e2e", "engine_identities": {"pagura": identity}}
        self.policy = {"maximum_output_bytes": 4096, "maximum_artifact_bytes": 100_000,
                       "timeout_seconds": 1, "measure_rss": False}
        self.store = ArtifactStore.for_report(self.report, self.policy, directory=self.directory)
        self.snapshot = self.store.snapshot(self.case)
        self.output = self.directory / "output.ppm"

    def context(self, pair=0, *, cases=None, output_paths=None, command=None):
        cases = cases or [self.case]
        command = command or [str(self.engine), "render", str(self.snapshot), str(self.output), "72", "0"]
        return self.store.freeze_execution(cases=cases, snapshot=self.snapshot, engine="pagura",
            comparison_engine="fake", pair=pair, order=["pagura", "fake"], command=command,
            output_paths=output_paths or {0: self.output})

    def capture(self, pair=0, *, raw=b"P6\n1 1\n255\nabc"):
        context = self.context(pair)
        self.store.before_launch(context, context["command"])
        self.output.write_bytes(raw)
        width, height, pixels = normalize(raw)
        result = {"status": "ok", "exit_code": 0, "elapsed_ns": 100,
                  "output_sha256": sha(raw), "pixels_sha256": sha(pixels), "width": width, "height": height}
        self.store.capture(context, result, {0: self.output}, page=0)
        return context, result

    def verify(self, context, result, **overrides):
        arguments = {"expected_context": context, "claimed_measurement": result,
                     "artifact_root": self.store.root, "policy": self.policy}
        arguments.update(overrides)
        return verify_artifact_binding(result["artifact_binding"], **arguments)

    def test_original_output_can_disappear_but_raw_bytes_are_reread(self):
        context, result = self.capture()
        self.output.unlink()
        verified = self.verify(context, result)
        self.assertEqual(verified["outputs"]["0"]["pixels_sha256"], sha(b"abc"))
        self.store.finish()
        self.assertEqual(self.report["artifact_run"]["status"], "complete")
        blob = self.store.root / verified["outputs"]["0"]["blob"]
        blob.chmod(0o600)
        blob.write_bytes(b"P6\n1 1\n255\nabd")
        with self.assertRaisesRegex(ValueError, "raw output"):
            self.verify(context, result)

    def test_warmup_and_sample_deduplicate_raw_blob_not_execution_identity(self):
        warm_context, warm = self.capture(-1)
        context, sample = self.capture(0)
        self.assertEqual(warm_context["phase"], "warmup")
        self.assertEqual(context["phase"], "sample")
        self.assertNotEqual(warm["artifact_binding"]["receipt"], sample["artifact_binding"]["receipt"])
        self.assertEqual(len(list((self.store.root / "blobs").iterdir())), 1)
        self.assertEqual(len(list((self.store.root / "receipts").iterdir())), 2)
        self.verify(warm_context, warm)
        self.verify(context, sample)
        with self.assertRaisesRegex(ValueError, "expected context"):
            self.verify(context, warm)

    def test_raw_identity_and_explicit_rgba_normalization_remain_distinct(self):
        raw = b"RGBA8 1 1 straight\n" + bytes([255, 0, 0, 128])
        context, result = self.capture(raw=raw)
        output = self.verify(context, result)["outputs"]["0"]
        self.assertEqual(output["normalization"], "rgb8-over-white-v1")
        self.assertEqual(output["raw_sha256"], sha(raw))
        self.assertEqual(output["pixels_sha256"], sha(bytes([255, 127, 127])))
        self.assertNotEqual(output["raw_sha256"], output["normalized_ppm_sha256"])

    def test_missing_receipt_or_blob_is_not_evidence(self):
        context, result = self.capture()
        receipt_path = self.store.root / result["artifact_binding"]["receipt"]
        receipt = json.loads(receipt_path.read_bytes())
        (self.store.root / receipt["outputs"]["0"]["blob"]).unlink()
        with self.assertRaises(ValueError):
            self.verify(context, result)
        receipt_path.unlink()
        with self.assertRaises(ValueError):
            self.verify(context, result)

    def test_changed_claimed_measurement_and_independent_context_are_refused(self):
        context, result = self.capture()
        for field, value in (("pixels_sha256", sha(b"abd")), ("width", 2), ("elapsed_ns", 1)):
            changed = {**result, field: value}
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "claimed measurement"):
                self.verify(context, changed)
        changed_context = {**context, "dpi": 144}
        with self.assertRaisesRegex(ValueError, "expected context"):
            self.verify(changed_context, result)

    def test_snapshot_is_owned_and_changed_snapshot_is_refused(self):
        context = self.context()
        self.source.write_bytes(b"original path changed after owned snapshot")
        self.store.before_launch(context, context["command"])
        self.snapshot.chmod(0o600)
        self.snapshot.write_bytes(b"changed actual input")
        with self.assertRaisesRegex(ValueError, "input snapshot"):
            self.store.before_launch(context, context["command"])
        self.assertEqual(self.report["artifact_run"]["status"], "incomplete")

    def test_changed_engine_or_unbound_command_refuses_process_start(self):
        context = self.context()
        self.engine.write_bytes(b"changed executable")
        with patch.object(measurement, "run_process") as process:
            result = measurement.measure(context["command"], self.output, None, self.policy,
                                         artifact_store=self.store, execution_context=context)
        process.assert_not_called()
        self.assertEqual(result["status"], "invalid-evidence")

    def test_wrong_command_prefix_and_unpinned_executable_are_refused(self):
        context = self.context(command=["unrelated", "render", str(self.snapshot), str(self.output), "72", "0"])
        with self.assertRaisesRegex(ValueError, "pinned engine command"):
            self.store.before_launch(context, context["command"])
        self.report["artifact_run"]["status"] = "recording"
        identity = self.report["engine_identities"]["pagura"]
        identity["artifacts"] = {str(self.source): sha(self.source.read_bytes())}
        context = self.context(pair=1)
        with self.assertRaisesRegex(ValueError, "absent from pinned"):
            self.store.before_launch(context, context["command"])

    def test_unsupported_pagura_options_cannot_claim_default_exact_settings(self):
        for index, flag in enumerate(("--accuracy", "--overprint", "--strict")):
            self.report["artifact_run"]["status"] = "recording"
            context = self.context(pair=index, command=[str(self.engine), "render", flag,
                str(self.snapshot), str(self.output), "72", "0"])
            with self.subTest(flag=flag), self.assertRaisesRegex(ValueError, "unexpected Pagura"):
                self.store.before_launch(context, context["command"])

    def test_options_hidden_in_pinned_engine_prefix_cannot_claim_exact(self):
        identity = self.report["engine_identities"]["pagura"]
        identity["command"] = [str(self.engine), "--accuracy", "competitive"]
        context = self.context(command=identity["command"] + ["render", str(self.snapshot), str(self.output), "72", "0"])
        with self.assertRaisesRegex(ValueError, "engine prefix"):
            self.store.before_launch(context, context["command"])
        self.assertEqual(context["render_settings"]["box"], "engine-default")

    def test_budget_exhaustion_is_explicit_and_stops_further_launches(self):
        self.store.maximum = self.report["artifact_run"]["stored_bytes"]
        context = self.context()
        self.output.write_bytes(b"P6\n1 1\n255\nabc")
        result = {"status": "ok", "exit_code": 0}
        self.store.capture(context, result, {0: self.output}, page=0)
        self.assertIn("artifact_error", result)
        self.assertNotIn("artifact_binding", result)
        self.assertEqual(self.report["artifact_run"]["status"], "incomplete")
        self.store.finish()
        self.assertEqual(self.report["artifact_run"]["status"], "incomplete")
        with self.assertRaisesRegex(ValueError, "further execution refused"):
            self.store.before_launch(context, context["command"])

    def test_budget_configuration_is_finite_positive_integer_and_input_bounded(self):
        for value in (False, 0, -1, 1.5, float("inf")):
            with self.subTest(value=value), self.assertRaises(ValueError):
                ArtifactStore.for_report({}, {**self.policy, "maximum_artifact_bytes": value},
                                         directory=self.directory)
        report = {"operation": "render-e2e", "engine_identities": self.report["engine_identities"]}
        store = ArtifactStore.for_report(report, {**self.policy, "maximum_input_bytes": 1},
                                         directory=self.directory)
        self.assertIsNone(store.snapshot(self.case))
        self.assertEqual(report["artifact_run"]["status"], "incomplete")

    def test_missing_and_malformed_outputs_are_retained_as_incomplete(self):
        context = self.context()
        result = {"status": "invalid-output", "exit_code": 0}
        self.store.capture(context, result, {0: self.output}, page=0)
        self.assertEqual(self.report["artifact_run"]["status"], "incomplete")
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.verify(context, result)
        self.output.write_bytes(b"not an image")
        self.report["artifact_run"]["status"] = "recording"
        other = self.context(1)
        malformed = {"status": "invalid-output", "exit_code": 0}
        self.store.capture(other, malformed, {0: self.output}, page=0)
        receipt = json.loads((self.store.root / malformed["artifact_binding"]["receipt"]).read_bytes())
        record = receipt["outputs"]["0"]
        self.assertEqual((self.store.root / record["blob"]).read_bytes(), b"not an image")
        self.assertIn("error", record)

    def test_document_receipt_binds_every_page_and_shared_measurement(self):
        cases = [{**self.case, "page": page} for page in range(2)]
        paths = {page: self.directory / f"page-{page + 1:04}.ppm" for page in range(2)}
        command = [str(self.engine), "convert", "--timings", str(self.snapshot), str(self.directory), "72", "0", "1"]
        context = self.context(cases=cases, command=command, output_paths=paths)
        self.store.before_launch(context, command)
        result = {"status": "ok", "exit_code": 0, "elapsed_ns": 900, "pages": []}
        for page, path in paths.items():
            raw = b"P6\n1 1\n255\n" + bytes([page, 20, 30])
            path.write_bytes(raw)
            result["pages"].append({"page": page, "status": "ok", "exit_code": 0,
                "width": 1, "height": 1, "pixels_sha256": sha(raw[-3:]), "output_sha256": sha(raw)})
        self.store.capture(context, result, paths, page=None)
        self.verify(context, result)
        for row in result["pages"]:
            self.assertEqual(self.verify(context, row)["measurement"]["page"], row["page"])
        changed = copy.deepcopy(result["pages"][0])
        changed["artifact_binding"]["page"] = 1
        with self.assertRaisesRegex(ValueError, "claimed measurement"):
            self.verify(context, changed)

    def test_reference_claim_requires_external_case_and_settings(self):
        context = self.context()
        context["references"] = {"0": {"sha256": "a" * 64, "settings": {}}}
        # Produce an authenticated-context-shaped receipt; self-digests alone
        # cannot supply the missing independent reference inputs to the verifier.
        receipt = {"schema": 1, "context": context, "measurement": {}, "outputs": {}}
        raw = json.dumps(receipt).encode()
        relative = self.store._store("receipts", "json", raw)
        binding = {"schema": 1, "run_id": context["run_id"], "context_sha256": digest(context),
                   "receipt": relative, "receipt_sha256": sha(raw), "page": 0}
        with self.assertRaisesRegex(ValueError, "independent reference inputs"):
            verify_artifact_binding(binding, expected_context=context, claimed_measurement={},
                artifact_root=self.store.root, policy=self.policy)

    def test_reviewed_reference_binding_uses_fresh_settings_and_native_ppm_only(self):
        # The authoritative fixture-generation check prepares these reviewed
        # pins before the Python tests. No product renderer is run here.
        case = next(case for case in render_cases() if case["id"] == "paint/image-direct-binary-halves")
        self.assertIn("render_references", case)
        self.case = {**case, "page": 0}
        self.snapshot = self.store.snapshot(self.case)
        raw = (ROOT / case["oracle"]).read_bytes()
        context, result = self.capture(raw=raw)
        settings = invocation_settings(case["sha256"], page=0, dpi=72,
            accuracy="exact", entrypoint="render_page", validation="full",
            render_policy_sha256=current_render_policy_sha256(ROOT))
        references = {0: {"case": case, "mode": "exact", "settings": settings}}
        verified = self.verify(context, result, references=references)
        self.assertEqual(verified["outputs"]["0"]["reference"]["status"], "ok")
        with self.assertRaisesRegex(ValueError, "independent reference inputs"):
            self.verify(context, result)
        wrong = {0: {**references[0], "settings": {**settings, "dpi": 144}}}
        with self.assertRaisesRegex(ValueError, "independently selected"):
            self.verify(context, result, references=wrong)
        width, height, pixels = normalize(raw)
        rgba = bytearray()
        for offset in range(0, len(pixels), 3):
            rgba.extend(pixels[offset:offset + 3] + b"\xff")
        self.store.output_limit = 8192
        context, result = self.capture(1, raw=f"RGBA8 {width} {height} straight\n".encode() + rgba)
        self.assertEqual(self.report["artifact_run"]["status"], "incomplete")
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.verify(context, result, references=references)

    def test_malformed_bindings_and_receipts_become_value_errors(self):
        context, result = self.capture()
        for binding in ([], None, {"schema": True}, {**result["artifact_binding"], "receipt": "../escape"}):
            with self.subTest(binding=binding), self.assertRaises(ValueError):
                verify_artifact_binding(binding, expected_context=context, claimed_measurement=result,
                    artifact_root=self.store.root, policy=self.policy)
        raw = b"[]"
        relative = self.store._store("receipts", "json", raw)
        binding = {**result["artifact_binding"], "receipt": relative, "receipt_sha256": sha(raw)}
        with self.assertRaises(ValueError):
            verify_artifact_binding(binding, expected_context=context, claimed_measurement=result,
                artifact_root=self.store.root, policy=self.policy)

    @unittest.skipUnless(hasattr(os, "mkfifo"), "POSIX FIFO regression")
    def test_bounded_reader_refuses_fifo_without_waiting_for_writer(self):
        path = self.directory / "fifo"
        os.mkfifo(path)
        with self.assertRaisesRegex(ValueError, "regular file"):
            read_bounded_regular(path, 10)
        with self.assertRaisesRegex(ValueError, "regular file"):
            hash_regular_file(path)

    def test_engine_hash_streaming_has_an_explicit_byte_bound(self):
        self.assertEqual(hash_regular_file(self.engine), sha(self.engine.read_bytes()))
        with self.assertRaisesRegex(ValueError, "byte budget"):
            hash_regular_file(self.engine, maximum=2)


if __name__ == "__main__":
    unittest.main()
