"""Independent integer-state oracle for the proposed matched ECS feature suite."""
import argparse
from dataclasses import dataclass, field
import hashlib
import json
from pathlib import Path
import struct

MASK = (1 << 32) - 1
SEED = 0x6D2B79F5
CASES = ("query_filter", "dirty_epoch", "archetype_churn", "dependency_dag", "buffer_fixed", "buffer_resize")
ROOT = Path(__file__).resolve().parents[4]


def mix(value):
    value &= MASK
    value = ((value ^ (value >> 16)) * 0x7FEB352D) & MASK
    value = ((value ^ (value >> 15)) * 0x846CA68B) & MASK
    return value ^ (value >> 16)


def update(value, identifier, tick):
    return (value * 1664525 + (1013904223 ^ mix(identifier + tick))) & MASK


@dataclass
class Row:
    """All authoritative state; capacity and scheduler bookkeeping are excluded."""
    identifier: int
    value: int
    tags: int = 0
    epoch: int = 0
    a: int = 0
    b: int = 0
    c: int = 0
    updates: int = 0
    words: list[int] = field(default_factory=list)


def selected(identifier, tick, count, percent, distribution):
    if distribution == "spread":
        return identifier % 100 < percent
    start = (tick * 997) % count
    return (identifier - start) % count < count * percent // 100


def initial(case, count, archetypes, length):
    rows = [Row(identifier, mix(identifier ^ SEED)) for identifier in range(count)]
    for row in rows:
        if case == "query_filter":
            row.tags = row.identifier & (archetypes - 1)
        if case.startswith("buffer_"):
            size = length if case == "buffer_fixed" else 4
            row.words = [mix((row.identifier * 64 + index) ^ SEED) for index in range(size)]
    return rows


def step(rows, case, tick, percent, distribution):
    for row in rows:
        identifier = row.identifier
        if case == "query_filter":
            if row.tags & 1 and not row.tags & 2:
                row.value = update(row.value, identifier, tick)
                row.updates += 1
        elif case == "dirty_epoch":
            if selected(identifier, tick, len(rows), percent, distribution):
                row.epoch = tick + 1
            if row.epoch == tick + 1:
                row.value = update(row.value, identifier, tick)
                row.updates += 1
        elif case == "archetype_churn":
            if identifier % 100 < percent:
                row.tags = 8 if tick % 2 == 0 else 0
            if row.tags & 8:
                row.value = update(row.value, identifier, tick)
                row.updates += 1
        elif case == "dependency_dag":
            row.a = mix(row.value + tick)
            row.b = (row.a * 3 + identifier) & MASK
            row.c = row.a ^ ((identifier * 5 + tick) & MASK)
            row.value = (row.b + row.c) & MASK
            row.updates += 1
        elif case.startswith("buffer_"):
            if case == "buffer_resize" and identifier % 100 < percent:
                length = 64 if tick % 2 == 0 else 4
                del row.words[length:]
                row.words.extend(mix((identifier * 64 + index) ^ SEED)
                                 for index in range(len(row.words), length))
            row.words = [update(value, identifier * 64 + index, tick)
                         for index, value in enumerate(row.words)]
            row.value = (row.value + sum(row.words)) & MASK
            row.updates += 1
        else:
            raise ValueError("unknown feature case")


def encode(rows, case, steps):
    """Version one: 24-byte header, then nine u32 fields and each row's words."""
    output = bytearray(struct.pack("<8sIIII", b"ARAECSF1", 1, CASES.index(case) + 1, len(rows), steps))
    for row in rows:
        output.extend(struct.pack("<9I", row.identifier, row.tags, row.value, row.epoch,
                                  row.a, row.b, row.c, row.updates, len(row.words)))
        for word in row.words:
            output.extend(struct.pack("<I", word))
    return bytes(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, required=True)
    parser.add_argument("--entities", type=int, default=64)
    parser.add_argument("--steps", type=int, default=3)
    parser.add_argument("--percent", type=int, default=10)
    parser.add_argument("--distribution", choices=["spread", "clustered"], default="spread")
    parser.add_argument("--archetypes", type=int, choices=[8, 64], default=8)
    parser.add_argument("--length", type=int, choices=[0, 4, 16, 64], default=4)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 0 < args.entities < 2**32 or not 0 <= args.steps < 2**32 or not 0 <= args.percent <= 100:
        parser.error("0 < entities < 2^32, 0 <= steps < 2^32 and percent in 0..100 required")
    destination = args.output.resolve()
    if not destination.is_relative_to((ROOT / "target").resolve()):
        parser.error("reference outputs must remain under this checkout's target/")
    if destination.exists():
        parser.error("output exists; choose a fresh path")
    rows = initial(args.case, args.entities, args.archetypes, args.length)
    for tick in range(args.steps):
        step(rows, args.case, tick, args.percent, args.distribution)
    data = encode(rows, args.case, args.steps)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(data)
    print(json.dumps({"case": args.case, "entities": len(rows), "steps": args.steps,
                      "updates": sum(row.updates for row in rows), "state_bytes": len(data),
                      "state_sha256": hashlib.sha256(data).hexdigest(), "output": str(destination)}))


if __name__ == "__main__":
    main()
