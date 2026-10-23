"""Interleave chunk sizes on the same optimized binary; run from crates/ara-anim."""
from pathlib import Path
from run import run

for round_index,order in enumerate([[8,16,32],[32,16,8],[16,8,32]]):
    for workers in [8,16]:
        for chunk in order:
            run([(Path("perf-local")/"candidate-stages.exe").resolve(),"300",str(workers),"all",str(chunk)],
                dict(round=round_index,engine="candidate",workers=workers,stage="all",chunk=chunk),
                Path("perf-local/chunks-final.jsonl"))
