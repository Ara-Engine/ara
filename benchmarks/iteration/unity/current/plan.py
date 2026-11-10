"""Print reproducible strict timed commands; never launch a process or prepare inputs."""
import argparse
import json
import sys

import s8_project as project
import smoke_plan


def s8_cases():
    for workload, count in (("w1", 1_000_000), ("w2", 20_000), ("w2", 50_000), ("w3", 1_000_000), ("w4", 100_000)):
        for runners in (1, 15):
            warmup = 3000 if workload == "w3" else 120
            yield {"case": workload, "variant": str(count), "runners": runners,
                   "flags": ["--workload", workload, "--count", str(count), "--runners", str(runners),
                             "--warmup", str(warmup), "--frames", "300", "--batch", "8192" if workload == "w3" else "4096"]}


def feature_cases(all_variants):
    variants = [("query_filter", ["--archetypes", str(a)], f"a{a}", 3) for a in (8, 64)]
    if all_variants:
        # 5/6-step resize plans collapse to one timed 420-step plan; parity is short.
        seen = set()
        for case, flags, label, _ in smoke_plan.variants():
            identity = (case, tuple(flags))
            if identity not in seen:
                seen.add(identity); variants.append((case, flags, label.split("_steps")[0], 3))
    else:
        variants += [("dirty_epoch", ["--percent", "1", "--distribution", "spread"], "p1_spread", 3),
                     ("archetype_churn", ["--percent", "10"], "p10", 3),
                     ("dependency_dag", ["--topology", "fork-join"], "fork-join", 3),
                     ("buffer_fixed", ["--length", "4"], "length4", 3),
                     ("buffer_fixed", ["--length", "64"], "length64", 3),
                     ("buffer_resize", ["--percent", "100"], "p100_reuse", 3),
                     ("buffer_resize", ["--percent", "100", "--trim"], "p100_trim", 3)]
    for case, flags, label, _ in variants:
        for runners in (1, 15):
            yield {"case": case, "variant": label, "runners": runners,
                   "flags": ["--case", case, "--count", "100000", "--runners", str(runners),
                             "--warmup", "120", "--frames", "300", *flags]}


def plan(suite, prefix, build_record=None, all_variants=False):
    cases = list(s8_cases() if suite == "s8" else feature_cases(all_variants))
    output = []
    for round_number in (1, 2):
        for case in (cases if round_number == 1 else reversed(cases)):
            label = f"{prefix}_r{round_number}_{case['case']}_{case['variant']}_runners{case['runners']}"
            common = [*case["flags"], "--run", label]
            if suite == "s8":
                if build_record: common += ["--build-record", str(build_record)]
                runner = project.HERE / "run.py"
                engines = ("ara", "unity") if round_number == 1 else ("unity", "ara")
                commands = [[sys.executable, str(runner), "run-" + engine, *common] for engine in engines]
                commands += [[sys.executable, str(runner), "verify", *common]]
            else:
                runner = project.features.HERE / "run.py"
                engines = ("bevy", "unity") if round_number == 1 else ("unity", "bevy")
                commands = [[sys.executable, str(runner), "run-" + engine, *common] for engine in engines]
                commands += [[sys.executable, str(runner), "verify", *common]]
            output.append({"round": round_number, "case": case["case"], "variant": case["variant"],
                           "runner_budget": case["runners"], "commands": commands})
    return {"launched": False, "suite": suite, "execution_lane": "timed", "fresh_process_rounds": 2,
            "pair_count": len(output), "requested_target_p50_fraction_lower": .20,
            "maximum_p95_regression_fraction": .05,
            "prerequisites": "parent CPU grant; explicit successful build/snapshots; owned prepare/bootstrap/reprepare/smoke; clear global recovery registry",
            "selection": "all unique feature configs" if all_variants and suite == "features" else "explicit representative matrix; not exhaustive feature timings",
            "pairs": output}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=("s8", "features"))
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--build-record")
    parser.add_argument("--all-variants", action="store_true")
    args = parser.parse_args()
    if not args.prefix or args.prefix in (".", "..") or any(char in args.prefix for char in "/\\"):
        parser.error("prefix must be one fresh folder-name prefix")
    print(json.dumps(plan(args.suite, args.prefix, args.build_record, args.all_variants), indent=2))


if __name__ == "__main__": main()
