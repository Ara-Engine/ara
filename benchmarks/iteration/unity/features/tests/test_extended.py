"""Independent full-state and backend-budget controls; no compiler or engine launch."""
from pathlib import Path
import struct
import sys
from types import SimpleNamespace
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import cases
import reference
import verify


class ExtendedTests(unittest.TestCase):
    def args(self, case, **values):
        settings = dict(case=case, count=1003, runners=15, warmup=2, frames=3, percent=10,
                        distribution="spread", length=4, trim=False, topology="chain", smoke=True)
        settings.update(values)
        return SimpleNamespace(**settings)

    def state(self, args):
        rows = reference.initial(args.case, args.count, 8, args.length)
        for tick in range(args.warmup + args.frames):
            reference.step(rows, args.case, tick, args.percent, args.distribution)
        return reference.encode(rows, args.case, args.warmup + args.frames)

    def test_backend_budgets_are_independent(self):
        for case in cases.NAMES[1:]:
            args = self.args(case)
            expected = 15 if case in ("dirty_epoch", "dependency_dag") else 14
            self.assertEqual(cases.workers(args), expected)
            args.runners = 1
            self.assertEqual(cases.workers(args), 0)

    def test_every_variant_has_full_scalar_state(self):
        variants = [("dirty_epoch", dict(percent=p, distribution=d)) for p in (0, 1, 10, 100) for d in ("spread", "clustered")]
        variants += [("archetype_churn", dict(percent=p)) for p in (1, 10, 100)]
        variants += [("dependency_dag", dict(topology=t)) for t in ("chain", "fork-join")]
        variants += [("buffer_fixed", dict(length=n)) for n in (0, 4, 16, 64)]
        variants += [("buffer_resize", dict(percent=p, trim=trim, frames=f)) for p in (10, 100) for trim in (False, True) for f in (3, 4)]
        for case, settings in variants:
            with self.subTest(case=case, **settings):
                args = self.args(case, **settings)
                verify.audit(self.state(args), args)

    def test_epoch_stage_and_live_buffer_corruption_are_rejected(self):
        for case, offset in [("dirty_epoch", 24 + 12), ("dependency_dag", 24 + 16), ("buffer_resize", 24 + 36)]:
            args = self.args(case)
            data = bytearray(self.state(args))
            data[offset] ^= 1
            with self.subTest(case=case), self.assertRaises(ValueError):
                verify.audit(data, args)

    def test_trailing_or_missing_payload_and_duplicate_ids_are_rejected(self):
        args = self.args("buffer_fixed", length=16)
        original = self.state(args)
        duplicate = bytearray(original)
        struct.pack_into("<I", duplicate, 24 + 36 + 16 * 4, 0)
        for data in [original[:-4], original + b"tail", duplicate]:
            with self.assertRaises(ValueError): verify.audit(data, args)

    def test_semantically_equal_capacity_and_topology_modes_have_distinct_inputs(self):
        reuse = self.args("buffer_resize")
        trim = self.args("buffer_resize", trim=True)
        self.assertEqual(self.state(reuse), self.state(trim))
        self.assertNotEqual(cases.encoded_config(reuse), cases.encoded_config(trim))
        self.assertNotEqual(cases.variant(reuse), cases.variant(trim))
        chain = self.args("dependency_dag")
        fork = self.args("dependency_dag", topology="fork-join")
        self.assertEqual(self.state(chain), self.state(fork))
        self.assertNotEqual(cases.encoded_config(chain), cases.encoded_config(fork))


if __name__ == "__main__": unittest.main()
