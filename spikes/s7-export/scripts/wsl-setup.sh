#!/usr/bin/env bash
# User-level toolchains for the S7 Linux lane inside WSL (idempotent, no sudo).
# clang/lld come from apt and need root: `wsl -d Ubuntu-24.04 -u root -e apt-get install -y clang lld`.
set -euo pipefail
if [ ! -x "$HOME/.dotnet/dotnet" ]; then
  curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh
  bash /tmp/dotnet-install.sh --version 10.0.202 --install-dir "$HOME/.dotnet"
fi
if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
  sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.99.1 --no-modify-path
fi
"$HOME/.dotnet/dotnet" --version
"$HOME/.cargo/bin/rustc" --version
