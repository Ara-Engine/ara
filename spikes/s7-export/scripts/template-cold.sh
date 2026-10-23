#!/usr/bin/env bash
# Cold build time of the runtime template per target, from a fresh target dir (Git Bash on
# Windows). In Ara these are CI artifacts; the numbers size the opt-in native lane.
#   bash scripts/template-cold.sh > results/template-cold.txt
set -uo pipefail
cd "$(dirname "$0")/.."
cargo build --release --features tools --bin zigcc >/dev/null 2>&1 || exit 1
zigcc="$(pwd -W)/target/release/zigcc.exe"
sdk="${ANDROID_HOME:-$LOCALAPPDATA/Android/Sdk}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}"
clang="$ndk/toolchains/llvm/prebuilt/windows-x86_64/bin/clang.exe"
export CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=out/template-cold
rm -rf out/template-cold

t() {
  local start=$(date +%s%N)
  "$@" >/dev/null 2>&1 || { echo "FAILED"; return; }
  echo "$(( ($(date +%s%N) - start) / 1000000 )) ms"
}
echo "windows-x64 ara-player: $(t cargo build --release --bin ara-player)"
echo "linux-x64 ara-player (zigcc, glibc 2.31): $(CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=$zigcc t cargo build --release --bin ara-player --target x86_64-unknown-linux-gnu)"
for triple in aarch64-linux-android x86_64-linux-android; do
  key=$(echo "$triple" | tr 'a-z-' 'A-Z_')
  export "CARGO_TARGET_${key}_LINKER=$clang" "CARGO_TARGET_${key}_RUSTFLAGS=-C link-arg=--target=${triple}29 -C link-arg=-Wl,-z,max-page-size=16384"
  echo "$triple libara_runtime.so: $(t cargo build --release --lib --target $triple)"
done
echo "(Ryzen 7 7800X3D, CARGO_BUILD_JOBS=4, dependencies included, other agents building concurrently)"
