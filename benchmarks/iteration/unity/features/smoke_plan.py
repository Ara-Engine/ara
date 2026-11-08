"""Print a bounded complete correctness plan; does not launch or build anything."""
import argparse
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
import cases
import project


def variants():
    for percent in (0, 1, 10, 100):
        for distribution in ("spread", "clustered"):
            yield "dirty_epoch", ["--percent", str(percent), "--distribution", distribution], "p%d_%s" % (percent, distribution), 3
    for percent in (1, 10, 100):
        yield "archetype_churn", ["--percent", str(percent)], f"p{percent}", 3
    for topology in ("chain", "fork-join"):
        yield "dependency_dag", ["--topology", topology], topology, 3
    for length in (0, 4, 16, 64):
        yield "buffer_fixed", ["--length", str(length)], f"length{length}", 3
    for percent in (10, 100):
        for trim in (False, True):
            for frames in (3, 4):
                flags = ["--percent", str(percent)] + (["--trim"] if trim else [])
                yield "buffer_resize", flags, f"p{percent}_{'trim' if trim else 'reuse'}_steps{frames + 2}", frames


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=cases.NAMES[1:])
    parser.add_argument("--prefix", default="extended-quality-01")
    args = parser.parse_args()
    if not args.prefix or args.prefix in (".", "..") or any(char in args.prefix for char in "/\\"):
        parser.error("prefix must be one fresh folder-name prefix")
    plan = []
    for case, flags, variant, frames in variants():
        if args.case and case != args.case: continue
        for runners in (1, 15):
            run = f"{args.prefix}_{case}_{variant}_r{runners}"
            reference = project.EXTENDED_PROJECT / "results" / run / "scalar-reference.bin"
            common = ["--case", case, "--count", "1003", "--runners", str(runners), "--warmup", "2", "--frames", str(frames), "--run", run, "--smoke", *flags]
            oracle_flags = flags if case in ("dirty_epoch", "archetype_churn", "buffer_fixed") else (["--percent", flags[1]] if case == "buffer_resize" else [])
            commands = [[sys.executable, str(project.HERE / "reference.py"), "--case", case,
                         "--entities", "1003", "--steps", str(frames + 2), "--output", str(reference), *oracle_flags]]
            commands += [[sys.executable, str(project.HERE / "run.py"), action, *common] for action in ("run-bevy", "run-unity")]
            commands.append([sys.executable, str(project.HERE / "run.py"), "verify", *common, "--reference", str(reference)])
            plan.append({"case": case, "variant": variant, "runner_budget": runners, "execution_lane": "correctness-smoke",
                         "timings_accepted": False, "commands": commands})
    print(json.dumps({"launched": False, "pair_count": len(plan), "prerequisites": "review + granted CPU slot, explicit extended build, isolated prepare/bootstrap/reprepare", "cases": plan}, indent=2))


if __name__ == "__main__": main()
