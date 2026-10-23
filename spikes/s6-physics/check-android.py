"""Compile-only check: Jolt + Ara's shim for aarch64-linux-android (NEON path) with NDK clang.

Same file list and defines as jolt-sys/build.rs, cross-platform-deterministic flags on.
Objects go to a temp dir and are discarded; nothing is linked or run (no Rust Android target
is installed on this PC). Usage: python check-android.py <ndk-root> [jobs]
"""
import concurrent.futures
import pathlib
import subprocess
import sys
import tempfile
import time

ndk = pathlib.Path(sys.argv[1])
jobs = int(sys.argv[2]) if len(sys.argv) > 2 else 4
here = pathlib.Path(__file__).resolve().parent
jolt = here / "vendor" / "JoltPhysics"
skip = {"ObjectStream.cpp", "ObjectStreamBinaryIn.cpp", "ObjectStreamBinaryOut.cpp", "ObjectStreamIn.cpp",
        "ObjectStreamOut.cpp", "ObjectStreamTextIn.cpp", "ObjectStreamTextOut.cpp", "TypeDeclarations.cpp"}
files = sorted(p for p in (jolt / "Jolt").rglob("*.cpp") if p.name not in skip)
files.append(here / "jolt-sys" / "shim" / "ara_jolt.cpp")
clang = ndk / "toolchains/llvm/prebuilt/windows-x86_64/bin/clang++.exe"
base = [str(clang), "--target=aarch64-linux-android24", "-std=c++17", "-O2", "-fno-rtti", "-fno-exceptions",
        "-DNDEBUG", "-DJPH_CROSS_PLATFORM_DETERMINISTIC", "-ffp-model=precise", "-ffp-contract=off",
        f"-I{jolt}", f"-I{here / 'jolt-sys' / 'shim'}", "-c"]

with tempfile.TemporaryDirectory() as tmp:
    def build(i_path):
        i, path = i_path
        out = pathlib.Path(tmp) / f"{i}.o"
        r = subprocess.run(base + [str(path), "-o", str(out)], capture_output=True, text=True)
        return path, r.returncode, r.stderr[-2000:], out.stat().st_size if out.exists() else 0

    start = time.time()
    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        results = list(pool.map(build, enumerate(files)))
    failed = [(p, err) for p, rc, err, _ in results if rc != 0]
    total = sum(size for *_, size in results)
    print(f"{len(files) - len(failed)}/{len(files)} files compiled for aarch64-linux-android24 "
          f"in {time.time() - start:.1f} s ({jobs} jobs), {total / 1e6:.1f} MB of objects")
    for p, err in failed:
        print(f"FAILED {p}\n{err}")
    sys.exit(1 if failed else 0)
