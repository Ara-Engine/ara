"""Summarize timestamp samples with standard-library Python; no packages needed."""
import json
import math
from pathlib import Path
import statistics

root = Path(__file__).parent / "evidence"
summary = {}
for name in ("headless", "window", "no-shadows"):
    run = json.loads((root / f"{name}.json").read_text(encoding="utf-8"))
    frames = run["render"]["frames"][10:]
    if not frames:
        raise ValueError(f"{name} needs more than 10 frames")
    passes = {}
    for key in ("compute_ms", "raster_ray_query_ms", "readback_ms"):
        values = sorted(frame[key] for frame in frames)
        passes[key] = {
            "median": statistics.median(values),
            "p95_nearest_rank": values[math.ceil(len(values) * 0.95) - 1],
            "min": values[0],
            "max": values[-1],
        }
    summary[name] = {
        "samples": len(frames),
        "discarded_warmup_frames": 10,
        "validation": run["validation"],
        "cache": run["cache"],
        "acceleration_build_gpu_ms": run["acceleration_build_gpu_ms"],
        "passes": passes,
    }
(root / "summary.json").write_text(
    json.dumps(summary, sort_keys=True, indent=2) + "\n", encoding="utf-8"
)
print(json.dumps(summary, sort_keys=True, indent=2))
