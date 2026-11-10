"""Source/control contracts; no Unity, Cargo, or compiler process starts here."""
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import s8_project as project
import binaries
import checks
import plan
import recovery
import monitor
import build as feature_build


class CurrentTests(unittest.TestCase):
    def test_export_adapter_changes_only_post_timing_region(self):
        source = (project.SOURCE / "Benchmark.cs").read_text()
        seam = "            var records = new float[count * 12];"
        derived = project.instrument_export(source)
        measured_end = source.index("            using var query")
        self.assertEqual(source[:measured_end], derived[:measured_end])
        self.assertIn("exact StableId epoch mismatch", derived)
        self.assertEqual(derived[derived.index(seam):], source[source.index(seam):])
        self.assertIn('File.Create(args[at + 6] + ".ids")', derived)
        with self.assertRaisesRegex(ValueError, "seam changed"):
            project.instrument_export(source.replace(seam, "changed"))

    def test_worker_ceilings_are_backend_specific(self):
        for workload in ("w1", "w2", "w3", "w4"):
            args = SimpleNamespace(workload=workload, runners=15)
            budget = project.budget(args)
            self.assertEqual(budget["ara"]["max_kernel_runners"], 15)
            self.assertEqual(budget["ara"]["argument_threads"], 15 if workload == "w4" else 14)
            self.assertEqual(budget["ara"]["caller_participates"], workload != "w4")
            self.assertEqual(budget["unity"]["expected_job_workers"], 14)

    def test_exact_ids_detect_epoch_errors_hidden_by_modulo_dump(self):
        args = SimpleNamespace(workload="w4", count=100_000, warmup=2, frames=3)
        ids = []
        for slot in range(args.count):
            group = slot // 10_000
            ids.append(100_000 + group * 10_000 + slot % 10_000 if group < 5 else slot)
        data = struct.pack("<" + "i" * len(ids), *ids)
        checks.identities(data, args)
        ids[0] += args.count
        with self.assertRaisesRegex(ValueError, "ID/epoch"):
            checks.identities(struct.pack("<" + "i" * len(ids), *ids), args)
        args.workload, args.count = "w1", 64
        checks.identities(struct.pack("<64i", *range(64)), args)

    def test_samples_are_recomputed_and_nan_rejected(self):
        row = {"ms": [2.0, 1.0, 3.0], "p50_ms": 2., "p95_ms": 3.}
        checks.check_samples(row, 3)
        row["p50_ms"] = 1.
        with self.assertRaisesRegex(ValueError, "percentile"): checks.check_samples(row, 3)
        row["ms"][0] = float("nan")
        with self.assertRaisesRegex(ValueError, "samples"): checks.check_samples(row, 3)

    def test_two_fresh_reversed_rounds_and_workload_defaults(self):
        result = plan.plan("s8", "test", "target/known.json")
        self.assertEqual(result["pair_count"], 20)
        first, second = result["pairs"][:10], result["pairs"][10:]
        for a, b in zip(first, reversed(second)):
            self.assertEqual((a["case"], a["variant"], a["runner_budget"]), (b["case"], b["variant"], b["runner_budget"]))
            self.assertEqual(a["commands"][0][2], "run-ara")
            self.assertEqual(b["commands"][0][2], "run-unity")
            ca, cb = a["commands"][0], b["commands"][0]
            self.assertNotEqual(ca[ca.index("--run") + 1], cb[cb.index("--run") + 1])
        self.assertEqual(project.COUNTS["w2"], 20_000)
        self.assertFalse(any("--smoke" in command for pair in result["pairs"] for command in pair["commands"]))
        features = plan.plan("features", "test")
        self.assertEqual({pair["case"] for pair in features["pairs"]}, set(project.features.cases.NAMES))

    def test_new_explicit_marker_cross_blocks_global_preferences(self):
        folder = project.ROOT / "target/current-harness-tests"; folder.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=folder) as temp:
            root = Path(temp)
            with patch.object(recovery, "ROOT", root), patch.object(recovery, "REGISTRY", root / "target/registry/projects.json"):
                s8 = root / "target/s8"; s8.mkdir(parents=True)
                (s8 / project.MARKER).write_text(json.dumps({"fixture": "s8-current-v1", "repository_root": str(root)}))
                recovery.register(s8)
                query = root / "target/query"; query.mkdir()
                (query / ".ara-query-feature.json").write_text(json.dumps({"fixture": "query_filter-v1", "repository_root": str(root)}))
                recovery.register(query)
                recovery.ensure_clear(s8)
                (s8 / recovery.PENDING).write_text("{}")
                with self.assertRaisesRegex(ValueError, "unresolved"): recovery.ensure_clear(query)
                (s8 / ".ara-query-feature.json").write_text("{}")
                with self.assertRaisesRegex(ValueError, "exactly one"): recovery.checked_project(s8)

    def test_build_manifest_requires_root_lock_and_every_kernel(self):
        with self.assertRaisesRegex(ValueError, "misses benchmark"):
            binaries.checked_sources({"source_sha256": {"Cargo.lock": "0" * 64}})

    def test_monitor_retains_midrun_contention(self):
        self.assertTrue(monitor.quiet([]))
        self.assertFalse(monitor.quiet([{"busy_tools": []}, {"busy_tools": [{"pid": 1}]}]))

    def test_transient_power_switch_cannot_reuse_matching_endpoints(self):
        ac, battery = {"power": "AC\n100%"}, {"power": "Battery\n99%"}
        self.assertTrue(monitor.power_stable(ac, [], ac))
        self.assertFalse(monitor.power_stable(ac, [battery], ac))
        self.assertFalse(monitor.power_stable(ac, [], battery))

    def test_feature_snapshot_archives_the_same_record_bytes_it_validates(self):
        folder = project.ROOT / "target/current-harness-tests"
        folder.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=folder) as temp:
            root = Path(temp)
            binary = root / "fixture"
            binary.write_bytes(b"immutable fixture")
            record = root / "build-provenance.json"
            original = json.dumps({"exit_code": 0, "source_sha256": {"source": "a"},
                "executable_sha256": project.sha(binary)}).encode()
            replacement = original.replace(b'"source": "a"', b'"source": "b"')
            record.write_bytes(original)
            read_bytes = Path.read_bytes
            reads = []
            def replace_after_read(path):
                raw = read_bytes(path)
                if path == record:
                    reads.append(raw)
                    record.write_bytes(replacement)
                return raw
            with patch.object(feature_build.project, "ROOT", root), \
                 patch.object(feature_build.project, "rust_target", return_value=root), \
                 patch.object(feature_build, "source_hashes", return_value={"source": "a"}), \
                 patch.object(Path, "read_bytes", replace_after_read):
                _, proof = feature_build.snapshot(binary)
            self.assertEqual(reads, [original])
            self.assertEqual(Path(proof["build_provenance"]).read_bytes(), original)
            self.assertEqual(record.read_bytes(), replacement)

    def test_monitor_samples_between_successful_waits_without_terminating_child(self):
        class Child:
            args = ["fake"]
            waits = 0
            def wait(self, timeout):
                self.waits += 1
                if self.waits == 1: raise subprocess.TimeoutExpired(self.args, timeout)
                return 0
        child = Child()
        status, observations = monitor.wait(child, 10, lambda strict: {"busy_tools": [{"pid": 3}]})
        self.assertEqual(status, 0)
        self.assertEqual(len(observations), 1)
        self.assertFalse(monitor.quiet(observations))


if __name__ == "__main__": unittest.main()
