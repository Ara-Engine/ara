"""Regression checks for source/copy drift; no compiler or engine is launched."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import plistlib
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import inputs
import project
import run as launcher
import verify as verification
import reference
import checkpoint_fixture
import recovery


class InputManifestTests(unittest.TestCase):
    def test_changed_timings_cannot_reuse_launch_evidence(self):
        result, _, _ = project.paths(self.args, "unity")
        result.parent.mkdir(parents=True, exist_ok=True)
        original = json.dumps({"p50_ms": 1, "p95_ms": 2}).encode()
        result.write_bytes(original)
        result.with_suffix(".launch.json").write_text(json.dumps({
            "result_sha256": hashlib.sha256(original).hexdigest()}))
        data, _ = verification.checked_result(result)
        self.assertEqual(data["p50_ms"], 1)
        result.write_text(json.dumps({"p50_ms": 0.1, "p95_ms": 0.2}))
        with self.assertRaisesRegex(ValueError, "result changed since"):
            verification.checked_result(result)

    def extended(self):
        self.args.case, self.args.percent, self.args.distribution = "dirty_epoch", 1, "clustered"
        self.args.length, self.args.trim, self.args.topology = 4, False, "chain"
        assets = self.here / "extended/unity/Assets"
        assets.mkdir(parents=True)
        (assets / "Extended.cs").write_text("class Extended {}\n")
        self.prepare()

    def test_extended_config_is_exactly_bound_and_cannot_change_mid_launch(self):
        self.extended()
        result, _, _ = project.paths(self.args, "unity")
        with patch.object(project.BASE, "active_editors", return_value=[]), \
             patch.object(project, "resources", return_value={"loadavg": [0, 0, 0], "power": "fixture power"}), \
             patch.object(launcher.subprocess, "Popen", return_value=self.engine_child()), \
             contextlib.redirect_stdout(io.StringIO()):
            launcher.run(self.args, "unity")
        launch = json.loads(result.with_suffix(".launch.json").read_text())
        case_input = launch["case_input"]
        self.assertEqual(hashlib.sha256(Path(case_input["path"]).read_bytes()).hexdigest(), case_input["sha256"])
        self.assertEqual(case_input["config"]["percent"], 1)
        verification.checked_case_input(launch, self.args)
        Path(case_input["path"]).write_text("{}")
        with self.assertRaisesRegex(ValueError, "case input changed since launch"):
            verification.checked_case_input(launch, self.args)
        self.args.run = "changed-input"
        child = self.engine_child(lambda: project.config_path(self.args).write_text("{}"))
        with patch.object(project.BASE, "active_editors", return_value=[]), \
             patch.object(project, "resources", return_value={"loadavg": [0, 0, 0], "power": "fixture power"}), \
             patch.object(launcher.subprocess, "Popen", return_value=child), \
             self.assertRaisesRegex(RuntimeError, "case input changed during execution"):
            launcher.run(self.args, "unity")

    def setUp(self):
        parent = project.ROOT / "target/feature-input-tests"
        parent.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=parent)
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name) / "repo"
        self.here = root / "benchmarks/iteration/unity/features"
        files = {"unity/Assets/Query.cs": "class Query {}\n", "unity/Assets/Features.asmdef": "{}\n",
                 "rust/src/main.rs": "fn main() {}\n", "rust/Cargo.toml": "[package]\nname='fixture'\n",
                 "generator.py": "value = 1\n", "editor/PreferenceRecovery.cs": "class PreferenceRecovery {}\n"}
        for name, data in files.items():
            path = self.here / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(data)
        (self.here.parent / "MacBatch.cs").write_text((Path(__file__).resolve().parents[2] / "MacBatch.cs").read_text())
        (self.here.parent / "bench.py").write_text("# fixture helper\n")
        editor = root / "editor/Unity.app/Contents/MacOS/Unity"
        editor.parent.mkdir(parents=True, exist_ok=True)
        editor.write_bytes(b"not an executable; never launched")
        with (editor.parents[1] / "Info.plist").open("wb") as stream:
            plistlib.dump({"CFBundleShortVersionString": "6000.4.5f1"}, stream)
        for owner, name, value in [(project, "HERE", self.here), (project, "ROOT", root), (project.BASE, "EDITOR", editor),
                (recovery, "ROOT", root), (recovery, "REGISTRY", root / "target/registry/projects.json")]:
            mocked = patch.object(owner, name, value)
            mocked.start()
            self.addCleanup(mocked.stop)
        self.args = SimpleNamespace(project=root / "target/project", count=64, archetypes=8, runners=1,
                                    warmup=2, frames=3, batch=4096, run="smoke", timeout=1)
        self.prepare()

    def prepare(self):
        with contextlib.redirect_stdout(io.StringIO()):
            project.prepare(self.args)

    def test_copied_inputs_are_all_guarded_and_repaired(self):
        names = ["Assets/Query.cs", "Assets/Features.asmdef", "Assets/Editor/MacBatch.cs",
                 "Assets/Editor/Features.Editor.asmdef", "Packages/manifest.json", "ProjectSettings/ProjectVersion.txt"]
        for name in names:
            with self.subTest(name=name):
                path = self.args.project / name
                original = path.read_bytes()
                path.write_bytes(original + b"tampered\n")
                with self.assertRaisesRegex(ValueError, "prepared Unity inputs changed"):
                    project.command(self.args, "unity")
                path.write_bytes(original)
        extra = self.args.project / "Assets/Unrecorded.cs"
        extra.write_text("class Unrecorded {}\n")
        with self.assertRaisesRegex(ValueError, "prepared Unity inputs changed"):
            project.command(self.args, "unity")
        self.prepare()
        self.assertFalse(extra.exists())
        project.command(self.args, "unity")

    def test_repository_templates_and_helper_changes_are_distinct(self):
        for path in [self.here / "unity/Assets/Query.cs", self.here / "generator.py",
                     self.here.parent / "MacBatch.cs", self.here.parent / "bench.py"]:
            with self.subTest(path=path.name):
                original = path.read_bytes()
                path.write_bytes(original + b"changed\n")
                with self.assertRaisesRegex(ValueError, "repository templates changed"):
                    project.command(self.args, "unity")
                path.write_bytes(original)

    def test_bootstrap_outputs_require_reprepare_then_are_guarded(self):
        before = project.prepared_manifest(self.args)
        for name in ["Assets/Query.cs.meta", "Packages/packages-lock.json", "ProjectSettings/ProjectSettings.asset"]:
            path = self.args.project / name
            path.write_text("generated fixture metadata\n")
        self.assertTrue(inputs.bootstrap_changes(before["input_manifest"], inputs.prepared(self.args.project)))
        with self.assertRaisesRegex(ValueError, "prepared Unity inputs changed"):
            project.command(self.args, "unity")
        self.prepare()
        project.command(self.args, "unity")
        meta = self.args.project / "Assets/Query.cs.meta"
        meta.write_text("changed after sealing\n")
        with self.assertRaisesRegex(ValueError, "prepared Unity inputs changed"):
            project.command(self.args, "unity")

    def engine_child(self, after_write=None):
        result, _, _ = project.paths(self.args, "unity")

        class Child:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return 0
            def wait(self, timeout):
                result.write_text(json.dumps({"full_ids_tags_updates_checked": True,
                                              "max_kernel_runners": 1, "p50_ms": 0, "p95_ms": 0}))
                environment = {
                    **checkpoint_fixture.initial(),
                    "finally_restored": True, "measurement_completed": True, "burst_enabled": True,
                    "burst_safety_checks": False, "burst_force_safety_checks": False, "burst_debug": False}
                checkpoint = checkpoint_fixture.checkpoint(result.parents[2], result, environment)
                checkpoint_fixture.receipt(result.parents[2], checkpoint)
                Path(str(result) + ".environment.json").write_text(json.dumps(environment))
                if after_write is not None:
                    after_write()
                return 0
        return Child()

    def test_launch_evidence_contains_complete_exact_manifest(self):
        expected = project.prepared_manifest(self.args)
        result, _, _ = project.paths(self.args, "unity")

        with patch.object(project.BASE, "active_editors", return_value=[]), \
             patch.object(project, "resources", return_value={"loadavg": [0, 0, 0], "power": "fixture power"}), \
             patch.object(launcher.subprocess, "Popen", return_value=self.engine_child()), \
             contextlib.redirect_stdout(io.StringIO()):
            launcher.run(self.args, "unity")
        launch = json.loads(result.with_suffix(".launch.json").read_text())
        self.assertEqual(launch["prepared_project"], expected)
        self.assertTrue(launch["prepared_inputs_unchanged_after_run"])
        self.assertEqual(expected["input_manifest_sha256"], inputs.manifest_digest(expected["input_manifest"]))
        marker = self.args.project / project.MARKER
        self.assertEqual(expected["marker_sha256"], hashlib.sha256(marker.read_bytes()).hexdigest())

    def test_first_import_samples_are_never_accepted(self):
        result, _, _ = project.paths(self.args, "unity")
        child = self.engine_child(lambda: (self.args.project / "Assets/Query.cs.meta").write_text("generated\n"))
        with patch.object(project.BASE, "active_editors", return_value=[]), \
             patch.object(project, "resources", return_value={"loadavg": [0, 0, 0], "power": "fixture power"}), \
             patch.object(launcher.subprocess, "Popen", return_value=child), \
             self.assertRaisesRegex(RuntimeError, "first-import metadata/settings"):
            launcher.run(self.args, "unity")
        self.assertFalse(result.with_suffix(".launch.json").exists())
        rejected = json.loads(result.with_suffix(".unaccepted.json").read_text())
        self.assertFalse(rejected["timings_accepted"])
        self.assertTrue(rejected["initial_import_bootstrap_outputs"])

    def test_smoke_bounds_and_artifacts_are_separate(self):
        timed, _, _ = project.paths(self.args, "unity")
        self.args.smoke = True
        smoke, _, _ = project.paths(self.args, "unity")
        self.assertNotEqual(timed, smoke)
        self.args.count = 10_001
        with self.assertRaisesRegex(ValueError, "correctness smoke requires"):
            project.execution_lane(self.args)
        self.args.count = 64
        self.args.warmup, self.args.frames = 8, 3
        with self.assertRaisesRegex(ValueError, "at most 10"):
            project.execution_lane(self.args)

    def test_busy_context_does_not_weaken_default_timing_guard(self):
        def output(command, **kwargs):
            return "42 200 /tools/bin/cargo\n" if command[0] == "ps" else "fixture power"
        with patch.object(project.subprocess, "check_output", side_effect=output), \
             patch.object(project.subprocess, "run", return_value=SimpleNamespace(stdout="cargo check fixture")):
            context = project.resources(strict=False)
            self.assertEqual(context["busy_tools"][0]["pid"], 42)
            self.assertFalse(context["strict_timing_guard"])
            with self.assertRaisesRegex(RuntimeError, "competing tool/test work"):
                project.resources()

    def test_smoke_records_busy_tools_but_never_emits_timing_summary(self):
        self.args.smoke = True
        result, _, _ = project.paths(self.args, "unity")
        context = {"loadavg": [4, 4, 4], "power": "fixture power", "busy_tools": [{"pid": 42}]}
        captured = io.StringIO()
        with patch.object(project.BASE, "active_editors", return_value=[]), \
             patch.object(project, "resources", return_value=context) as resources, \
             patch.object(launcher.subprocess, "Popen", return_value=self.engine_child()), \
             contextlib.redirect_stdout(captured):
            launcher.run(self.args, "unity")
        self.assertTrue(all(call.kwargs == {"strict": False} for call in resources.call_args_list))
        launch = json.loads(result.with_suffix(".launch.json").read_text())
        self.assertFalse(launch["timings_accepted"])
        self.assertEqual(launch["before"]["busy_tools"], [{"pid": 42}])
        summary = json.loads(captured.getvalue())
        self.assertNotIn("p50_ms", summary)
        self.assertNotIn("p95_ms", summary)
        self.args.smoke = False
        with self.assertRaisesRegex(ValueError, "smoke/unaccepted evidence"):
            verification.checked_lane({"unity": launch}, self.args)

    def test_quiet_smoke_quality_can_pass_without_becoming_timing_evidence(self):
        self.args.smoke = True
        self.args.reference = self.args.project / "results/reference.bin"
        rows = reference.initial("query_filter", self.args.count, self.args.archetypes, 4)
        for tick in range(self.args.warmup + self.args.frames):
            reference.step(rows, "query_filter", tick, 10, "spread")
        state = reference.encode(rows, "query_filter", self.args.warmup + self.args.frames)
        self.args.reference.parent.mkdir(parents=True, exist_ok=True)
        self.args.reference.write_bytes(state)
        prepared = project.prepared_manifest(self.args)
        for engine in ["bevy", "unity"]:
            result, dump, _ = project.paths(self.args, engine)
            result.parent.mkdir(parents=True, exist_ok=True)
            dump.write_bytes(state)
            result.write_text(json.dumps({"count": self.args.count, "archetypes": self.args.archetypes,
                "warmup": self.args.warmup, "frames": self.args.frames, "batch": self.args.batch,
                "full_ids_tags_updates_checked": True, "max_kernel_runners": 1,
                "state_blake3": "fixture-digest", "p50_ms": 0, "p95_ms": 0}))
            environment = Path(str(result) + ".environment.json")
            environment.write_text(json.dumps({"finally_restored": True, "measurement_completed": True,
                "burst_enabled": True, "burst_safety_checks": False,
                "burst_force_safety_checks": False, "burst_debug": False}))
            result.with_suffix(".launch.json").write_text(json.dumps({
                "result_sha256": hashlib.sha256(result.read_bytes()).hexdigest(),
                "environment_sha256": hashlib.sha256(environment.read_bytes()).hexdigest(),
                "execution_lane": "correctness-smoke", "timings_accepted": False,
                "strict_timing_guard_enforced": False, "prepared_project": prepared,
                "prepared_inputs_unchanged_after_run": True, "resource_guards_passed": True,
                "before": {"power": "same quiet profile"}}))
        library = self.args.project / "fixture-library.rlib"
        library.write_bytes(b"mock cached library; no compiler launched")
        captured = io.StringIO()
        with patch.object(project.BASE, "cached_tool", return_value=(library, [])), \
             patch.object(verification.subprocess, "check_output", return_value="fixture-digest\n"), \
             contextlib.redirect_stdout(captured):
            verification.verify(self.args)
        report = json.loads(captured.getvalue())
        self.assertTrue(report["quality_passed"])
        self.assertTrue(report["independent_scalar_reference_verified"])
        self.assertFalse(report["timings_accepted"])
        self.assertFalse(report["timing_comparable"])


if __name__ == "__main__":
    unittest.main()
