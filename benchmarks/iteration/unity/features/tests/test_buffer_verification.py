"""Offline verification must keep the compiled buffer variant bound to its launch."""
import copy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build
import project
import verify


class BufferVerificationTests(unittest.TestCase):
    def packet(self, directory, kernel="simd", legacy=False):
        args = SimpleNamespace(case="buffer_fixed", buffer_kernel=kernel)
        record = {**copy.deepcopy(project.build_command(args)), "exit_code": 0,
                  "executable_sha256": "ab" * 32}
        if not legacy:
            record.update(buffer_kernel_requested=kernel, buffer_kernel_contract_version=1)
        if kernel == "simd":
            record.update(rustc_target_cfg=['target_arch="aarch64"', 'target_feature="neon"'],
                          rustc_target_cfg_command=["rustc", "--print", "cfg", "-C", "target-cpu=native"],
                          cargo_build_target=None)
        raw = json.dumps(record).encode()
        path = directory / "archived.build.json"
        path.write_bytes(raw)
        proof = build.checked_build_settings(record, args)
        executable = {"build_provenance": str(path), "build_provenance_sha256": hashlib.sha256(raw).hexdigest(),
                      "executable_sha256": record["executable_sha256"]}
        reported = proof["expected_reported_label"]
        results = {"bevy": {}, "unity": {}}
        launches = {"bevy": {"executable": executable}, "unity": {}}
        if not legacy:
            executable["buffer_kernel"] = proof
            results["bevy"]["buffer_kernel"] = reported
            for engine, launch in launches.items():
                launch.update(buffer_kernel_requested=kernel,
                              buffer_kernel_reported=reported if engine == "bevy" else None)
        return args, results, launches, path

    def test_archived_build_result_and_launch_must_agree(self):
        parent = project.ROOT / "target/buffer-verification-tests"
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            args, results, launches, path = self.packet(Path(temporary))
            proof = verify.checked_buffer_kernel(results, launches, args)
            self.assertEqual(proof["reported"], "contiguous-neon4")
            for mutate in (
                lambda r, l: r["bevy"].update(buffer_kernel="contiguous-scalar-fallback"),
                lambda r, l: l["unity"].update(buffer_kernel_requested="baseline"),
                lambda r, l: l["bevy"].update(buffer_kernel_reported="contiguous-scalar"),
                lambda r, l: l["bevy"]["executable"].update(executable_sha256="cd" * 32),
            ):
                wrong_results, wrong_launches = copy.deepcopy((results, launches))
                mutate(wrong_results, wrong_launches)
                with self.assertRaises(ValueError):
                    verify.checked_buffer_kernel(wrong_results, wrong_launches, args)
            path.write_bytes(path.read_bytes() + b"\n")
            with self.assertRaisesRegex(ValueError, "provenance changed"):
                verify.checked_buffer_kernel(results, launches, args)

    def test_legacy_baseline_is_distinct_from_a_new_missing_label(self):
        parent = project.ROOT / "target/buffer-verification-tests"
        parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=parent) as temporary:
            args, results, launches, _ = self.packet(Path(temporary), "baseline", legacy=True)
            self.assertTrue(verify.checked_buffer_kernel(results, launches, args)["legacy_metadata_allowed"])
            args, results, launches, _ = self.packet(Path(temporary), "baseline")
            results["bevy"].clear()
            with self.assertRaisesRegex(ValueError, "reported buffer kernel"):
                verify.checked_buffer_kernel(results, launches, args)


if __name__ == "__main__": unittest.main()
