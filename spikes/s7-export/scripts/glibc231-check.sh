#!/usr/bin/env bash
# (`--root` is needed: under an explicit loader, /proc/self/exe is ld.so.)
# Runs exported Linux builds against Debian 11's glibc 2.31 (the Steam Runtime 3 "sniper"
# baseline) through its own dynamic loader. No root, no container. Usage (inside WSL):
#   bash scripts/glibc231-check.sh <dist dir>...
set -uo pipefail
dists=(); for d in "$@"; do dists+=("$(realpath "$d")"); done
root=/tmp/glibc231
if [ ! -e "$root/sysroot/usr/lib/x86_64-linux-gnu/libstdc++.so.6" ]; then
  mkdir -p "$root/debs" && cd "$root/debs"
  for f in pool/main/g/glibc/libc6_2.31-13+deb11u11_amd64.deb pool/main/g/gcc-10/libgcc-s1_10.2.1-6_amd64.deb pool/main/g/gcc-10/libstdc++6_10.2.1-6_amd64.deb; do
    curl -fsSLO "http://deb.debian.org/debian/$f"
  done
  for d in *.deb; do dpkg-deb -x "$d" "$root/sysroot"; done
fi
libs="$root/sysroot/lib/x86_64-linux-gnu:$root/sysroot/usr/lib/x86_64-linux-gnu"
ld="$root/sysroot/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"
"$ld" --library-path "$libs" "$root/sysroot/lib/x86_64-linux-gnu/libc.so.6" | head -1
for dist in "${dists[@]}"; do
  echo "== $dist"
  rm -rf "/tmp/glibc231-run" && cp -r "$dist" /tmp/glibc231-run && cd /tmp/glibc231-run && chmod +x ara-player
  "$ld" --library-path "$libs" ./ara-player --root "$PWD" --ticks 30 2>&1 | grep -E "ara_script_init|verify:|GLIBC|not found|error" | cut -c1-200
  echo "exit=${PIPESTATUS[0]}"
done
