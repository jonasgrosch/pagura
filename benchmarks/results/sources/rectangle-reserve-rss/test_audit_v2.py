"""Bounded framework-launcher regression tests; no executable is launched."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("rectangle_rss_audit_v2", Path(__file__).with_name("audit-v2.py"))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class PythonLaunchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        version = self.root / "Cellar/python@3.14/3.14.7/Frameworks/Python.framework/Versions/3.14"
        self.runtime = version / "bin/python3.14"
        self.actual = version / "Resources/Python.app/Contents/MacOS/Python"
        self.requested = self.root / "bin/python3"
        for path in (self.runtime, self.actual, self.requested):
            path.parent.mkdir(parents=True, exist_ok=True)
        self.runtime.write_bytes(b"independent launcher bytes")
        self.actual.write_bytes(b"independent app executable bytes")
        self.requested.symlink_to(self.runtime)
        command = [str(self.requested), "-B", str(self.root / "rss_aa.py"),
                   "--root", str(self.root), "--bindings", str(self.root / "bindings.json"),
                   "--bindings-sha256", "a" * 64, "--output", str(self.root / "result")]
        launch = {"requested_argv0": str(self.requested), "actual_pin_timing": "post-run",
                  "runtime_executable": {"path": str(self.runtime), "sha256": audit.sha(self.runtime.read_bytes())},
                  "actual_argv0": {"path": str(self.actual), "sha256": audit.sha(self.actual.read_bytes())}}
        self.pins = {"producer_argv": command, "python_launch": launch}
        self.plan = {"runtime": {"executable": str(self.runtime)},
                     "bound_files_sha256": {str(self.runtime): launch["runtime_executable"]["sha256"]}}
        self.report = {"producer_argv": [str(self.actual), *command[1:]]}

    def test_framework_handoff_keeps_distinct_launcher_and_app_pins(self):
        self.assertNotEqual(self.report["producer_argv"][0], self.pins["producer_argv"][0])
        self.assertNotEqual(self.pins["python_launch"]["runtime_executable"]["sha256"],
                            self.pins["python_launch"]["actual_argv0"]["sha256"])
        self.assertNotIn(str(self.actual), self.plan["bound_files_sha256"])
        audit.verify_python_launch(self.pins, self.plan, self.report)

    def test_every_argument_after_argv0_remains_exact(self):
        for index in range(1, len(self.report["producer_argv"])):
            report = copy.deepcopy(self.report)
            report["producer_argv"][index] += "-changed"
            with self.subTest(index=index), self.assertRaises(ValueError):
                audit.verify_python_launch(self.pins, self.plan, report)
        for command in (self.report["producer_argv"][:-1], [*self.report["producer_argv"], "--retry"]):
            with self.subTest(command=command), self.assertRaises(ValueError):
                audit.verify_python_launch(self.pins, self.plan, {"producer_argv": command})

    def test_unpinned_observed_or_requested_argv0_fails(self):
        report = copy.deepcopy(self.report)
        report["producer_argv"][0] = str(self.runtime)
        with self.assertRaises(ValueError):
            audit.verify_python_launch(self.pins, self.plan, report)
        pins = copy.deepcopy(self.pins)
        pins["python_launch"]["requested_argv0"] = str(self.runtime)
        with self.assertRaises(ValueError):
            audit.verify_python_launch(pins, self.plan, self.report)

    def test_framework_version_prefix_and_suffix_cannot_change(self):
        for old, new in (("Versions/3.14", "Versions/3.15"),
                         ("Cellar/python@3.14", "other/python@3.14"),
                         ("Contents/MacOS/Python", "Contents/MacOS/other")):
            pins, report = copy.deepcopy(self.pins), copy.deepcopy(self.report)
            changed = str(self.actual).replace(old, new)
            pins["python_launch"]["actual_argv0"]["path"] = changed
            report["producer_argv"][0] = changed
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                audit.verify_python_launch(pins, self.plan, report)

    def test_current_launcher_and_app_bytes_must_match_separate_pins(self):
        for path in (self.runtime, self.actual):
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.subTest(path=path), self.assertRaises(ValueError):
                audit.verify_python_launch(self.pins, self.plan, self.report)
            path.write_bytes(original)

    def test_requested_launcher_resolution_and_recorded_runtime_must_match(self):
        self.requested.unlink()
        self.requested.symlink_to(self.actual)
        with self.assertRaises(ValueError):
            audit.verify_python_launch(self.pins, self.plan, self.report)
        self.requested.unlink()
        self.requested.symlink_to(self.runtime)
        plan = copy.deepcopy(self.plan)
        plan["runtime"]["executable"] = str(self.actual)
        with self.assertRaises(ValueError):
            audit.verify_python_launch(self.pins, plan, self.report)

    def test_original_launcher_pin_and_post_run_scope_are_required(self):
        plan = copy.deepcopy(self.plan)
        plan["bound_files_sha256"][str(self.runtime)] = "0" * 64
        with self.assertRaises(ValueError):
            audit.verify_python_launch(self.pins, plan, self.report)
        plan = copy.deepcopy(self.plan)
        plan["bound_files_sha256"][str(self.actual)] = self.pins["python_launch"]["actual_argv0"]["sha256"]
        with self.assertRaises(ValueError):
            audit.verify_python_launch(self.pins, plan, self.report)
        for timing in ("pre-run", None, True):
            pins = copy.deepcopy(self.pins)
            pins["python_launch"]["actual_pin_timing"] = timing
            with self.subTest(timing=timing), self.assertRaises(ValueError):
                audit.verify_python_launch(pins, self.plan, self.report)

    def test_missing_or_malformed_external_pin_fails(self):
        pins = copy.deepcopy(self.pins)
        del pins["python_launch"]
        with self.assertRaises(ValueError):
            audit.verify_python_launch(pins, self.plan, self.report)
        for key, value in (("runtime_executable", None), ("actual_argv0", {"path": str(self.actual)})):
            pins = copy.deepcopy(self.pins)
            pins["python_launch"][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                audit.verify_python_launch(pins, self.plan, self.report)


if __name__ == "__main__":
    unittest.main()
