"""Preference crash contracts using owned temporary files and mocked Unity children."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import checkpoint_fixture as fixture
import project
import recovery
import recover
import run as launcher


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        folder = project.ROOT / "target/feature-recovery-tests"
        folder.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=folder); self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name, value in [("ROOT", self.root), ("REGISTRY", self.root / "target/shared/projects.json")]:
            mocked = patch.object(recovery, name, value); mocked.start(); self.addCleanup(mocked.stop)
        self.project = self.make_project("ufx", "extended-v1")
        self.result = self.project / "results/run/value_unity_smoke.json"
        self.args = SimpleNamespace(project=self.project, checkpoint=None, timeout=1)

    def make_project(self, name, family):
        root = self.root / "target" / name; root.mkdir(parents=True)
        (root / ".ara-query-feature.json").write_text(json.dumps({"repository_root": str(self.root), "fixture": family}))
        recovery.register(root)
        return root

    def checkpoint(self):
        return fixture.checkpoint(self.project, self.result)

    def test_write_failure_before_mutation_has_no_guessed_recovery(self):
        self.result.parent.mkdir(parents=True)
        state = recovery.failed(self.project, self.result, 1)
        self.assertEqual(state["stage"], "unknown_no_checkpoint")
        self.assertIsNone(state["checkpoint"])
        with self.assertRaisesRegex(ValueError, "unresolved"):
            recovery.ensure_clear(self.project)
        with self.assertRaisesRegex(ValueError, "explicit --checkpoint"):
            recover.command(self.args)

    def test_native_crash_preserves_checkpoint_and_blocks_another_project(self):
        checkpoint = self.checkpoint()
        state = recovery.failed(self.project, self.result, -11)
        self.assertFalse(state["restoration_proven"])
        self.assertTrue(checkpoint.exists())
        query = self.make_project("uf", "query_filter-v1")
        with self.assertRaisesRegex(ValueError, "before any owned feature"):
            recovery.ensure_clear(query)

    def test_checkpoint_scan_blocks_even_if_failure_marker_write_was_lost(self):
        self.checkpoint()
        query = self.make_project("query", "query_filter-v1")
        self.assertFalse((self.project / recovery.PENDING).exists())
        with self.assertRaisesRegex(ValueError, "unresolved durable"):
            recovery.ensure_clear(query)

    def test_exact_five_key_whitelist_rejects_extra_missing_and_duplicate(self):
        for change in [lambda rows: rows.pop(), lambda rows: rows.append(dict(rows[0])),
                       lambda rows: rows[0].update(key="UnrelatedUserPreference")]:
            initial = fixture.initial(); change(initial["initial_editor_preferences"])
            path = fixture.checkpoint(self.project, self.result, initial)
            with self.assertRaisesRegex(ValueError, "exactly five"):
                recovery.checked_checkpoint(self.project, path)

    def test_checkpoint_initial_option_types_and_state_hash_are_checked(self):
        initial = fixture.initial(); initial[recovery.OPTIONS[0]] = "false"
        path = fixture.checkpoint(self.project, self.result, initial)
        with self.assertRaisesRegex(ValueError, "initial options"):
            recovery.checked_checkpoint(self.project, path)
        path = self.checkpoint(); data = json.loads(path.read_text()); data["initial_json"] += " "
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "initial-state bytes"):
            recovery.checked_checkpoint(self.project, path)

    def test_owned_project_result_and_symlink_boundaries(self):
        path = self.checkpoint()
        with self.assertRaisesRegex(ValueError, "another invocation"):
            recovery.checked_checkpoint(self.project, path, self.result.with_name("other.json"))
        other = self.make_project("other", "extended-v1")
        with self.assertRaisesRegex(ValueError, "owned scratch results"):
            recovery.checked_checkpoint(other, path)
        alias = self.result.parent / "alias.initial.json"; alias.symlink_to(path)
        with self.assertRaisesRegex(ValueError, "owned scratch"):
            recovery.checked_checkpoint(self.project, alias)

    def test_success_receipt_matches_exact_initial_options_and_preferences(self):
        path = self.checkpoint(); fixture.receipt(self.project, path)
        initial = fixture.initial(); environment = dict(initial, finally_restored=True)
        proof = recovery.successful(self.project, self.result, environment)
        self.assertTrue(proof["restored"])
        recovery.ensure_clear(self.project)
        environment[recovery.OPTIONS[0]] = True
        with self.assertRaisesRegex(ValueError, "initial state differs"):
            recovery.successful(self.project, self.result, environment)

    def test_stale_or_incomplete_receipts_never_clear_checkpoint(self):
        path = self.checkpoint(); receipt = fixture.receipt(self.project, path, restored=False)
        with self.assertRaisesRegex(ValueError, "not complete"):
            recovery.checked_receipt(self.project, path, receipt)
        fixture.receipt(self.project, path); data = json.loads(receipt.read_text()); data["invocation_id"] = "b" * 32
        receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "not bound"):
            recovery.checked_receipt(self.project, path, receipt)
        with self.assertRaisesRegex(ValueError, "unresolved durable"):
            recovery.ensure_clear(self.project)

    def test_recovery_command_has_no_benchmark_and_binds_pending_checkpoint(self):
        path = self.checkpoint(); recovery.failed(self.project, self.result, -11); self.args.checkpoint = path
        with patch.object(project, "prepared_manifest", return_value={}):
            command, _, _, _, _ = recover.command(self.args)
        self.assertIn("AraFeatureControl.PreferenceRecovery.Recover", command)
        self.assertNotIn("-ara-feature", command)
        self.assertEqual(command[-2], hashlib.sha256(path.read_bytes()).hexdigest())
        self.assertEqual(len(command[-1]), 32)
        data = json.loads(path.read_text()); data["invocation_id"] = "b" * 32; path.write_text(json.dumps(data))
        with patch.object(project, "prepared_manifest", return_value={}), self.assertRaisesRegex(ValueError, "does not match pending"):
            recover.command(self.args)

    def child(self, path, restored=True):
        owner = self
        class Child:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return 0
            def wait(self, timeout):
                attempt = json.loads((owner.project / recovery.PENDING).read_text())["recovery_attempt_id"]
                fixture.receipt(owner.project, path, "explicit_recovery", restored, attempt)
                return 0
        return Child()

    def test_mocked_recovery_clears_only_matching_pending_invocation(self):
        path = self.checkpoint(); recovery.failed(self.project, self.result, -11); self.args.checkpoint = path
        other = self.make_project("second", "query_filter-v1")
        other_result = other / "results/run/other.json"; fixture.checkpoint(other, other_result); recovery.failed(other, other_result, -11)
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", return_value=self.child(path)), contextlib.redirect_stdout(io.StringIO()):
            recover.run(self.args)
        self.assertFalse((self.project / recovery.PENDING).exists())
        self.assertTrue((other / recovery.PENDING).exists())
        with self.assertRaisesRegex(ValueError, "unresolved"):
            recovery.ensure_clear(self.project)

    def test_mocked_failed_recovery_keeps_global_block(self):
        path = self.checkpoint(); recovery.failed(self.project, self.result, -11); self.args.checkpoint = path
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", return_value=self.child(path, False)), self.assertRaisesRegex(ValueError, "not complete"):
            recover.run(self.args)
        self.assertTrue((self.project / recovery.PENDING).exists())

    def timeout_child(self, create_checkpoint=False):
        owner = self
        class Child:
            stopped = False
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return -15 if self.stopped else None
            def terminate(self): self.stopped = True
            def wait(self, timeout):
                if self.stopped: return -15
                if create_checkpoint: owner.checkpoint()
                raise subprocess.TimeoutExpired(["mocked Unity"], timeout)
        return Child()

    def check_launcher_timeout(self, create_checkpoint):
        self.args.smoke = True
        self.args.count, self.args.warmup, self.args.frames = 64, 0, 1
        paths = (["mocked Unity"], {}, self.result, self.result.with_suffix(".bin"), self.result.with_suffix(".log"))
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "command", return_value=paths), \
             patch.object(project, "prepared_manifest", return_value={}), patch.object(project, "resources", return_value={}), \
             patch.object(launcher.subprocess, "Popen", return_value=self.timeout_child(create_checkpoint)), \
             self.assertRaises(subprocess.TimeoutExpired):
            launcher.run(self.args, "unity")
        pending = json.loads((self.project / recovery.PENDING).read_text())
        self.assertEqual(pending["stage"], "after_checkpoint_mutation_possible" if create_checkpoint else "unknown_no_checkpoint")
        with self.assertRaisesRegex(ValueError, "unresolved"):
            recovery.ensure_clear(self.project)

    def test_launcher_timeout_before_checkpoint_blocks_future_launches(self):
        self.check_launcher_timeout(False)

    def test_launcher_timeout_after_checkpoint_preserves_known_initial_state(self):
        self.check_launcher_timeout(True)

    def check_recovery_failure_with_old_normal_receipt(self, timeout):
        path = self.checkpoint(); fixture.receipt(self.project, path); self.args.checkpoint = path
        recovery.ensure_clear(self.project)
        class Crashed:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return -11
            def wait(self, timeout): return -11
        child = self.timeout_child() if timeout else Crashed()
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", return_value=child), \
             self.assertRaises(subprocess.TimeoutExpired if timeout else RuntimeError):
            recover.run(self.args)
        state = json.loads((self.project / recovery.PENDING).read_text())
        self.assertEqual(state["stage"], "explicit_recovery_started")
        self.assertEqual(state["checkpoint"]["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())
        with self.assertRaisesRegex(ValueError, "unresolved"):
            recovery.ensure_clear(self.project)

    def test_recovery_crash_is_not_resolved_by_old_normal_receipt(self):
        self.check_recovery_failure_with_old_normal_receipt(False)

    def test_recovery_timeout_is_not_resolved_by_old_normal_receipt(self):
        self.check_recovery_failure_with_old_normal_receipt(True)

    def test_recovery_receipt_must_match_new_attempt(self):
        path = self.checkpoint()
        receipt = fixture.receipt(self.project, path, "explicit_recovery", attempt="a" * 32)
        with self.assertRaisesRegex(ValueError, "another recovery attempt"):
            recovery.checked_receipt(self.project, path, receipt, "b" * 32)

    def test_recovery_commands_use_fresh_attempt_paths_and_short_ipc_directory(self):
        self.args.checkpoint = self.checkpoint()
        with patch.object(project, "prepared_manifest", return_value={}):
            first = recover.command(self.args); second = recover.command(self.args)
        self.assertNotEqual(first[0][-1], second[0][-1])
        for command, environment, checkpoint, receipt, log in [first, second]:
            attempt = command[-1]
            self.assertEqual(receipt, Path(str(checkpoint) + ".recovery-" + attempt + ".json"))
            self.assertEqual(log, Path(str(checkpoint) + ".recovery-" + attempt + ".log"))
            self.assertIn(Path(environment["TMPDIR"]).name, ["uf-t", "ufx-t"])

    def test_valid_attempt_specific_receipt_resolves_checkpoint_scan(self):
        path = self.checkpoint()
        fixture.receipt(self.project, path, "explicit_recovery", attempt="a" * 32)
        recovery.ensure_clear(self.project)

    def test_attempt_receipt_filename_cannot_disagree_with_its_record(self):
        path = self.checkpoint()
        receipt = fixture.receipt(self.project, path, "explicit_recovery", attempt="a" * 32)
        moved = Path(str(path) + ".recovery-" + "b" * 32 + ".json"); receipt.rename(moved)
        with self.assertRaisesRegex(ValueError, "path/kind differs"):
            recovery.checked_receipt(self.project, path, moved)
        with self.assertRaisesRegex(ValueError, "unresolved durable"):
            recovery.ensure_clear(self.project)

    def test_previous_failed_attempt_logs_receipts_and_pending_are_retained(self):
        path = self.checkpoint(); self.args.checkpoint = path
        owner = self
        class Crashed:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return -11
            def wait(self, timeout): return -11
        def crash(argv, **kwargs):
            Path(argv[argv.index("-logFile") + 1]).write_text("first failure preserved\n")
            return Crashed()
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", side_effect=crash), self.assertRaises(RuntimeError):
            recover.run(self.args)
        previous = json.loads((self.project / recovery.PENDING).read_text()); first = previous["recovery_attempt_id"]
        first_log = Path(str(path) + ".recovery-" + first + ".log")
        old_receipt = fixture.receipt(self.project, path, "explicit_recovery", False, first)
        old_bytes = old_receipt.read_bytes()
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", return_value=self.child(path)), contextlib.redirect_stdout(io.StringIO()):
            recover.run(self.args)
        self.assertEqual(first_log.read_text(), "first failure preserved\n")
        self.assertEqual(old_receipt.read_bytes(), old_bytes)
        launches = list(path.parent.glob(path.name + ".recovery-*.launch.json"))
        self.assertEqual(len(launches), 1)
        proof = json.loads(launches[0].read_text())
        self.assertNotEqual(proof["pending_failure"]["recovery_attempt_id"], first)
        self.assertEqual(proof["pending_failure"]["previous_failure"], previous)
        recovery.ensure_clear(self.project)

    def test_completed_old_attempt_cannot_clear_a_newer_pending_attempt(self):
        path = self.checkpoint(); self.args.checkpoint = path; owner = self
        class Changed:
            def __enter__(self): return self
            def __exit__(self, *args): return False
            def poll(self): return 0
            def wait(self, timeout):
                pending = owner.project / recovery.PENDING
                state = json.loads(pending.read_text())
                fixture.receipt(owner.project, path, "explicit_recovery", attempt=state["recovery_attempt_id"])
                state["recovery_attempt_id"] = "b" * 32; pending.write_text(json.dumps(state)); return 0
        with patch.object(project.BASE, "active_editors", return_value=[]), patch.object(project, "prepared_manifest", return_value={}), \
             patch.object(recover.subprocess, "Popen", return_value=Changed()), self.assertRaisesRegex(ValueError, "does not resolve"):
            recover.run(self.args)
        self.assertEqual(json.loads((self.project / recovery.PENDING).read_text())["recovery_attempt_id"], "b" * 32)

    def test_derived_wrapper_never_sets_options_when_checkpoint_write_failed(self):
        base = (Path(__file__).resolve().parents[2] / "MacBatch.cs").read_text()
        derived = recovery.adapt_wrapper(base)
        self.assertLess(derived.index("PreferenceRecovery.Save("), derived.index("featureMutationStarted = true"))
        self.assertLess(derived.index("featureMutationStarted = true"), derived.index("options.EnableBurstCompilation = true"))
        guard = derived.index("if (featureMutationStarted)\n")
        self.assertLess(guard, derived.index("options.EnableBurstCompilation = initialCompilation"))
        self.assertLess(guard, derived.index("options.EnableBurstDebug = initialDebug"))
        self.assertIn("if (featureMutationStarted) RestorePreferences(initialPreferences);", derived)
        self.assertIn("if (featureMutationStarted) AraFeatureControl.PreferenceRecovery.Complete(", derived)
        with self.assertRaisesRegex(ValueError, "seam changed"):
            recovery.adapt_wrapper(base.replace("options.EnableBurstCompilation = true;", "changed();"))


if __name__ == "__main__": unittest.main()
