"""Windows interleaved benchmarks, recording host CPU minus benchmark CPU per run.

python benchmarks/run.py --frames 300 --rounds 3 --output perf-local/results.jsonl
Executables are preserved before/after builds; no compiler runs in this driver.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import subprocess
import time

KERNEL = ctypes.WinDLL("kernel32", use_last_error=True)
KERNEL.GetSystemTimes.argtypes = [ctypes.POINTER(wintypes.FILETIME)] * 3
KERNEL.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4

def ticks(value):
    return value.dwLowDateTime | value.dwHighDateTime << 32

def system_times():
    values = [wintypes.FILETIME() for _ in range(3)]
    if not KERNEL.GetSystemTimes(*(ctypes.byref(v) for v in values)):
        raise ctypes.WinError(ctypes.get_last_error())
    return [ticks(v) for v in values]

def process_ticks(process):
    values = [wintypes.FILETIME() for _ in range(4)]
    if not KERNEL.GetProcessTimes(int(process._handle), *(ctypes.byref(v) for v in values)):
        raise ctypes.WinError(ctypes.get_last_error())
    return ticks(values[2]) + ticks(values[3])

def run(command, metadata, output):
    # Keep the process handle open until after GetProcessTimes on exit.
    before = system_times()
    start = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, creationflags=subprocess.BELOW_NORMAL_PRIORITY_CLASS)
    stdout, stderr = process.communicate()
    cpu = process_ticks(process)
    after = system_times()
    idle, kernel, user = [b-a for a,b in zip(before,after)]
    total = kernel + user
    row = dict(metadata, command=[str(c) for c in command], seconds=time.monotonic()-start,
        total_cpu_percent=100*(total-idle)/total,
        background_cpu_percent=100*max(0,total-idle-cpu)/total,
        stdout=stdout.strip(), stderr=stderr.strip(), exit_code=process.returncode)
    with output.open("a", encoding="utf-8", newline="\n") as file:
        file.write(json.dumps(row)+"\n")
    print(json.dumps(row), flush=True)
    if process.returncode:
        raise RuntimeError("benchmark failed")

def crowd(command, metadata, output):
    before = system_times()
    previous_cpu = 0
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, creationflags=subprocess.BELOW_NORMAL_PRIORITY_CLASS)
    info = []
    for line in process.stdout:
        fields = line.strip().split(",")
        if fields[0] not in ["animation","animation+palette"]:
            info.append(line.strip())
            continue
        after = system_times()
        cpu = process_ticks(process)
        idle,kernel,user = [b-a for a,b in zip(before,after)]
        total = kernel+user
        row = dict(metadata,workers=int(fields[1]),stage=fields[0],p50_ms=float(fields[4]),p95_ms=float(fields[5]),
            total_cpu_percent=100*(total-idle)/total,
            background_cpu_percent=100*max(0,total-idle-(cpu-previous_cpu))/total,info=info)
        with output.open("a",encoding="utf-8",newline="\n") as file:
            file.write(json.dumps(row)+"\n")
        print(json.dumps(row),flush=True)
        before,previous_cpu=after,cpu
    stderr=process.stderr.read()
    if process.wait():
        raise RuntimeError(stderr)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--round-offset", type=int, default=0)
    parser.add_argument("--workers", type=int, nargs="+", default=[1,8,16])
    parser.add_argument("--stages", nargs="+", default=["sample","blend","model","palette","all"])
    parser.add_argument("--engines", nargs="+", default=["baseline","candidate","ozz"])
    parser.add_argument("--chunk", type=int, default=16)
    parser.add_argument("--crowd", action="store_true")
    parser.add_argument("--output", type=Path, default=Path("perf-local/results.jsonl"))
    args = parser.parse_args()
    for round_index in range(args.rounds):
        round_id=args.round_offset+round_index
        if args.crowd:
            order=["baseline","candidate"] if round_id%2==0 else ["candidate","baseline"]
            for workers in args.workers:
                for engine in order:
                    executable=engine+"-crowd.exe"
                    crowd([(Path("perf-local")/executable).resolve(),str(args.frames),str(workers),str(args.chunk)],
                        dict(round=round_id,engine=engine,chunk=args.chunk),args.output)
            continue
        for workers in args.workers:
            for stage in args.stages:
                order = args.engines if round_id % 2 == 0 else args.engines[::-1]
                for engine in order:
                    executable = Path("perf-local") / ("ozz-build/Release/ozz_bench.exe" if engine == "ozz" else engine+"-stages.exe")
                    command = [executable.resolve()]
                    if engine == "ozz":
                        command.append(Path("perf-local/assets.json").resolve())
                    command += [str(args.frames),str(workers),stage,str(args.chunk)]
                    run(command, dict(round=round_id,engine=engine,workers=workers,stage=stage,chunk=args.chunk),args.output)

if __name__ == "__main__":
    main()
