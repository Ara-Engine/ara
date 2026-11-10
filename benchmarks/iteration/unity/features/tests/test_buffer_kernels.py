"""Feature selection and provenance contracts; no compiler or Unity invocation."""
import copy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import project
import build


class BufferKernelTests(unittest.TestCase):
    def args(self, kernel="baseline", case="buffer_fixed"):
        return SimpleNamespace(buffer_kernel=kernel, case=case, project=project.EXTENDED_PROJECT,
                               count=1003, runners=15, warmup=2, frames=3, smoke=True,
                               length=64, percent=100, trim=False, distribution="spread",
                               topology="chain", batch=4096, run="probe")

    def record(self, args, cfg=None):
        settings = project.build_command(args)
        record = {**copy.deepcopy(settings), "buffer_kernel_requested": project.buffer_kernel(args),
                  "buffer_kernel_contract_version": 1}
        if cfg is not None:
            record.update(rustc_target_cfg=cfg,
                          rustc_target_cfg_command=["rustc", "--print", "cfg", "-C", "target-cpu=native"],
                          cargo_build_target=None)
        return record

    def test_default_command_environment_and_outputs_are_unchanged(self):
        args = self.args()
        del args.buffer_kernel
        settings = project.build_command(args)
        self.assertEqual(settings["command"], ["/usr/bin/nice", "-n", "10", "cargo", "build", "--locked",
            "--release", "--bin", "ara-ecs-extended-fixture", "--manifest-path", str(project.HERE / "rust/Cargo.toml")])
        self.assertEqual(settings["environment"], {"CARGO_TARGET_DIR": str(project.EXTENDED_RUST_TARGET),
            "CARGO_BUILD_JOBS": "2", "RUSTFLAGS": "-C target-cpu=native", "CARGO_PROFILE_RELEASE_LTO": "thin",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1", "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
            "CARGO_PROFILE_RELEASE_DEBUG": "0", "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS": "false"})
        self.assertEqual(project.paths(args, "bevy")[0].name, "buffer_fixed_1003_length64_r15_bevy_smoke.json")
        self.assertEqual(project.rust_target(self.args(case="query_filter")), project.RUST_TARGET)

    def test_opt_in_features_targets_and_outputs_are_distinct(self):
        targets, outputs = set(), set()
        for kernel in project.BUFFER_KERNELS:
            args = self.args(kernel)
            settings = project.build_command(args)
            targets.add(project.rust_target(args)); outputs.add(project.paths(args, "bevy")[0])
            if kernel != "baseline":
                self.assertEqual(settings["command"][-2:], ["--features", "buffer_" + kernel])
                self.assertEqual(project.rust_target(args).name, "extended-feature-rust-" + kernel)
                self.assertIn("_kernel_" + kernel + "_unity", project.paths(args, "unity")[0].name)
        self.assertEqual(len(targets), 3)
        self.assertEqual(len(outputs), 3)

    def test_nonbuffer_cases_and_unknown_kernels_are_rejected(self):
        for case in ("query_filter", "dirty_epoch", "archetype_churn", "dependency_dag"):
            for kernel in ("contiguous", "simd"):
                with self.assertRaisesRegex(ValueError, "require buffer_fixed or buffer_resize"):
                    project.build_command(self.args(kernel, case))
        with self.assertRaisesRegex(ValueError, "unknown buffer kernel"):
            project.buffer_kernel(self.args("unproven"))
        project.buffer_kernel(self.args("simd", "buffer_resize"))

    def test_wrong_command_environment_or_feature_record_is_rejected(self):
        args = self.args("contiguous")
        record = self.record(args)
        for mutate in (lambda r: r["command"].extend(["--features", "buffer_simd"]),
                       lambda r: r["environment"].update(RUSTFLAGS="-C target-cpu=generic"),
                       lambda r: r.update(buffer_kernel_requested="simd"),
                       lambda r: r.update(buffer_kernel_contract_version=2)):
            invalid = copy.deepcopy(record); mutate(invalid)
            with self.assertRaises(ValueError): build.checked_build_settings(invalid, args)
        with self.assertRaisesRegex(ValueError, "command/environment/features"):
            build.checked_build_settings(self.record(self.args()), args)

    def test_simd_label_requires_actual_target_architecture_and_neon_proof(self):
        args = self.args("simd")
        for cfg, expected in [(['target_arch="aarch64"', 'target_feature="neon"'], "contiguous-neon4"),
                              (['target_arch="aarch64"'], "contiguous-scalar-fallback"),
                              (['target_arch="x86_64"', 'target_feature="sse2"'], "contiguous-scalar-fallback")]:
            proof = build.checked_build_settings(self.record(args, cfg), args)
            self.assertEqual(proof["expected_reported_label"], expected)
            executable = {"buffer_kernel": proof}
            self.assertEqual(build.checked_result_kernel({"buffer_kernel": expected}, executable, args), expected)
            wrong = "contiguous-neon4" if expected.endswith("fallback") else "contiguous-scalar-fallback"
            with self.assertRaisesRegex(ValueError, "reported buffer kernel"):
                build.checked_result_kernel({"buffer_kernel": wrong}, executable, args)
        with self.assertRaisesRegex(ValueError, "target-feature proof"):
            build.checked_build_settings(self.record(args), args)

    def test_new_baseline_requires_label_and_legacy_record_stays_identifiable(self):
        args = self.args()
        proof = build.checked_build_settings(self.record(args), args)
        with self.assertRaisesRegex(ValueError, "reported buffer kernel"):
            build.checked_result_kernel({}, {"buffer_kernel": proof}, args)
        legacy = project.build_command(args)
        proof = build.checked_build_settings(legacy, args)
        self.assertTrue(proof["legacy_metadata_allowed"])
        self.assertIsNone(build.checked_result_kernel({}, {"buffer_kernel": proof}, args))

    def test_snapshot_rejects_wrong_variant_before_binary_read_or_run(self):
        parent = project.ROOT / "target/buffer-kernel-contract-tests"
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            target = Path(temporary)
            record = self.record(self.args())
            record.update(exit_code=0, source_sha256={"frozen": "source"}, executable_sha256=hashlib.sha256(b"not-run").hexdigest())
            (target / "build-provenance.json").write_text(json.dumps(record))
            with patch.object(project, "rust_target", return_value=target), \
                 patch.object(build, "source_hashes", return_value=record["source_sha256"]), \
                 self.assertRaisesRegex(ValueError, "command/environment/features"):
                build.snapshot(target / "absent-binary", self.args("contiguous"))


if __name__ == "__main__": unittest.main()
