#!/usr/bin/env bash
# S7 measurement series for the desktop lanes (Git Bash on Windows). Writes results/.
# Android needs a device or emulator for its verify stage; see RESULT.md for that run.
#
#   bash scripts/measure.sh
#
# Starts WSL (Ubuntu-24.04) for the Linux lanes; stop it afterwards with
# `wsl --terminate Ubuntu-24.04` if it was stopped before.
set -uo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS=4
cargo build --release --features tools --bins || exit 1
T=./target/release/ara-s7.exe
mkdir -p results

$T doctor > results/doctor.txt
$T doctor --json > results/doctor.ndjson

# Cold = C# obj/bin, out/<lane> and dist/<lane> removed; NuGet and cargo caches warm
# (in Ara the template is a prebuilt download, so its cargo build is not export work).
for lane in "windows aot" "windows r2r" "linux aot" "linux aot-cross" "linux r2r"; do
  set -- $lane
  echo "== export $1 $2"
  $T export --platform "$1" --lane "$2" --clean --json > "results/export-$1-$2-cold.ndjson"
  $T export --platform "$1" --lane "$2" --json > "results/export-$1-$2-warm.ndjson"
done

# Start-up and boundary cost: 11 spawns (first = coldest without admin rights) + one --bench run.
$T bench-start dist/windows-aot --json > results/start-windows-aot.ndjson
$T bench-start dist/windows-r2r --json > results/start-windows-r2r.ndjson
# Linux numbers run natively in WSL on ext4 copies made by each lane's audit stage.
for lane in linux-aot linux-aot-cross linux-r2r; do
  wsl.exe -d Ubuntu-24.04 -e bash -lc "cd ~/ara-s7 && ./target/release/ara-s7 bench-start dist/$lane --json" > "results/start-$lane.ndjson"
done

# Pack determinism across hosts: the Linux-built tool must produce the Windows bytes.
$T pack content out/determinism/windows.arapack --json > results/pack-determinism.ndjson
wsl.exe -d Ubuntu-24.04 -e bash -lc "cd ~/ara-s7 && ./target/release/ara-s7 pack content /tmp/linux.arapack --json" >> results/pack-determinism.ndjson
