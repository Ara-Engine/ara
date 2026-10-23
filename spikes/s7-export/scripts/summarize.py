"""Prints the RESULT.md tables from results/*.ndjson. Usage: python -I scripts/summarize.py"""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent / "results"
LANES = [
    ("windows", "aot"),
    ("windows", "r2r"),
    ("linux", "aot"),
    ("linux", "aot-cross"),
    ("linux", "r2r"),
    ("android", "aot"),
]


def events(path):
    if not path.exists():
        return []
    return [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l.startswith("{")]


def result(path):
    r = [e for e in events(path) if e["type"] == "result"]
    return r[0]["result"] if r else None


def mb(b):
    return f"{b / 1e6:.2f} MB" if b >= 1e6 else f"{b / 1e3:.1f} KB"


def s(ms):
    return f"{ms / 1000:.1f} s" if ms >= 1000 else f"{ms:.0f} ms"


def export_tables():
    print("| Lane | Built on | Cold total | Warm total | Cold script (C#) | Warm script | Template (warm cargo) | Pack | Verify |")
    print("|---|---|---|---|---|---|---|---|---|")
    for plat, lane in LANES:
        cold = result(ROOT / f"export-{plat}-{lane}-cold.ndjson")
        warm = result(ROOT / f"export-{plat}-{lane}-warm.ndjson")
        if not cold or not warm:
            continue
        st = lambda r, n: next((x["ms"] for x in r["stages"] if x["stage"] == n), 0)
        built = cold.get("built_on", "windows" if plat != "linux" else "?")
        ok = cold["verify"]["pass"] if cold.get("verify") else "n/a"
        print(
            f"| {plat} {lane} | {built} | {s(cold['total_ms'])} | {s(warm['total_ms'])} | {s(st(cold, 'script'))} | "
            f"{s(st(warm, 'script'))} | {s(st(warm, 'template'))} | {s(st(cold, 'pack'))} | {ok} |"
        )
    print()
    print("| Lane | Template | C# part | Pack | Total folder |")
    print("|---|---|---|---|---|")
    for plat, lane in LANES:
        r = result(ROOT / f"export-{plat}-{lane}-warm.ndjson")
        if not r:
            continue
        z = r["sizes"]
        if plat == "android":
            for a in z["per_abi"]:
                print(f"| android {a['abi']} | {mb(a['template_bytes'])} | {mb(a['script_bytes'])} | {mb(z['pack_bytes'])} | n/a |")
            print(f"| android APK (2 ABIs) / AAB | | | | {mb(z['apk_bytes'])} / {mb(z['aab_bytes'])} |")
            print(f"| android per-device download (bundletool get-size) | | | | {', '.join(z['per_device_apks'][1:])} |")
        else:
            print(
                f"| {plat} {lane} | {mb(z['template_bytes'])} | {mb(z['script_bytes'])} | {mb(z['pack_bytes'])} | {mb(z['total_bytes'])} |"
            )


def start_tables():
    print("| Lane | Cold: spawn → first C# call | Warm median: spawn → first C# call | Warm: script load | Warm process wall | Rust→C# | C#→Rust | C#→Rust (SuppressGCTransition) | C# tick (100k) | Rust tick (100k) |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for name in ["windows-aot", "windows-r2r", "linux-aot", "linux-aot-cross", "linux-r2r"]:
        r = result(ROOT / f"start-{name}.ndjson")
        if not r:
            continue
        c, w, b, t = r["cold"], r["warm_median"], r["bench"], r["tick"]
        print(
            f"| {name} | {c['spawn_to_first_cs_call_ms']:.1f} ms | {w['spawn_to_first_cs_call_ms']:.1f} ms | {w['script_load_ms']:.1f} ms | "
            f"{w['process_wall_ms']:.1f} ms | {b['rust_to_cs_ns']:.2f} ns | {b['cs_to_rust_ns']:.2f} ns | {b['cs_to_rust_fast_ns']:.2f} ns | "
            f"{t['cs_min_ms']:.3f} ms ({t['cs_ns_per_entity']:.2f} ns/e) | {t['rust_ns_per_entity'] * 0.1:.3f} ms ({t['rust_ns_per_entity']:.2f} ns/e) |"
        )


def warnings():
    print("| Lane | Warnings / errors in the cold export |")
    print("|---|---|")
    for plat, lane in LANES:
        ev = events(ROOT / f"export-{plat}-{lane}-cold.ndjson")
        codes = [f"{e['type']} {e['code']}" for e in ev if e["type"] in ("warning", "error")]
        print(f"| {plat} {lane} | {', '.join(codes) or 'none'} |")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    {"exports": export_tables, "start": start_tables, "warnings": warnings}.get(
        sys.argv[1] if len(sys.argv) > 1 else "", lambda: [f() or print() for f in (export_tables, start_tables, warnings)]
    )()
