"""Aggregate the measured Rust 1.99.1 comparison without rerunning any workload."""
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent / "artifacts" / "rust199"


def read(name):
    return json.loads((ROOT / name).read_text(encoding="utf-8-sig"))


summary = {"builds": read("builds.json"), "stacks": {}}
for stack in ("renet", "lightyear"):
    reports = [read(f"{stack}-{mode}-{profile}.json") for mode, profile in (
        ("dedicated3", "impaired"), ("dedicated4", "impaired"),
        ("listen", "impaired"), ("dedicated4-live", "impaired"), ("dedicated4", "clean"))]
    primary = [report for report in reports if report["mode"] != "dedicated4-live"]
    local = read(f"local-{stack}.json")
    gates = {
        "mode_parity": all(report["ideal_script_outcome_match"] for report in primary),
        "late_join": all(report["late_join_pass"] for report in reports),
        "conditioner_convergence": all(report["convergence_pass"] for report in reports),
        "replay": all(report["replay_pass"] and report["server"]["state_hash"] == report["server"]["replay_hash"] and report["server"]["replay_tick_checkpoints"] == 600 for report in reports),
        "privacy": all(report["privacy_pass"] and all(client["state"]["foreign_inventory_values"] == 0 for client in report["clients"]) for report in reports),
        "local_zero_serialization": all(local[field] == 0 for field in ("gameplay_bytes_serialized", "network_resources", "transport_objects", "prediction_history_entries")),
    }
    assert all(gates.values()), (stack, gates)
    measured = reports[1]
    clients = measured["clients"]
    magnitudes = [value for client in clients for value in client["state"]["corrections_mm"]]
    stress = reports[3]
    stress_magnitudes = [value for client in stress["clients"] for value in client["state"]["corrections_mm"]]
    tree = set(line.replace(" (*)", "").strip() for line in (ROOT / f"dependencies-{stack}.txt").read_text(encoding="utf-8-sig").splitlines())
    summary["stacks"][stack] = {
        "gates": gates,
        "bandwidth_clients": [{"player": client["player"], "down_bytes_s": client["downstream_udp_bytes_per_second"], "up_bytes_s": client["upstream_udp_bytes_per_second"]} for client in clients],
        "mean_down_bytes_s": statistics.mean(client["downstream_udp_bytes_per_second"] for client in clients),
        "mean_up_bytes_s": statistics.mean(client["upstream_udp_bytes_per_second"] for client in clients),
        "nominal_corrections": len(magnitudes),
        "nominal_prediction_checks": sum(client["state"]["checked_prediction_checkpoints"] for client in clients),
        "stress_corrections": len(stress_magnitudes),
        "stress_mean_mm": statistics.mean(stress_magnitudes) if stress_magnitudes else 0,
        "stress_max_mm": max(stress_magnitudes, default=0),
        "stress_checks": sum(client["state"]["checked_prediction_checkpoints"] for client in stress["clients"]),
        "stress_ideal_outcome_match": stress["ideal_script_outcome_match"],
        "stress_native_diagnostics": [client["state"]["native"] for client in stress["clients"]],
        "late_join_ticks": {report["mode"]: report["clients"][-1]["state"]["first_baseline_tick"] for report in reports if report["mode"] != "dedicated3" and report["profile"] != "loopback, no impairment"},
        "normal_dependency_packages": len(tree),
    }
summary["local_benchmarks"] = {}
for stack in ("renet", "lightyear", "local"):
    report = read(f"bench-{stack}.json")
    summary["local_benchmarks"][stack] = {"bare_ns": statistics.median(report["plain_ns_per_tick"]), "direct_ns": statistics.median(report["direct_simulation_ns_per_tick"]), "local_ns": statistics.median(report["local_ns_per_tick"]), "local_direct_ratio": report["median_local_to_direct_ratio"]}
renet, lightyear = (summary["stacks"][name] for name in ("renet", "lightyear"))
summary["lightyear_traffic_reduction_percent"] = {"down": 100 * (1 - lightyear["mean_down_bytes_s"] / renet["mean_down_bytes_s"]), "up": 100 * (1 - lightyear["mean_up_bytes_s"] / renet["mean_up_bytes_s"])}
(ROOT / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8", newline="\n")
print("All six gates pass for both stacks. Wrote artifacts/rust199/summary.json")
