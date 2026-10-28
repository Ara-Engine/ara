set -eu
cd /home/islam/ara-ux-editor
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=6 CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_BUILD_DIR="$PWD/target"
export DOTNET_CLI_HOME="$PWD/target/ux-p4/dotnet-home"
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/bench-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
if nice -n 10 cargo test -p ara-editor --lib gizmo_hit_before_after_benchmark --locked -- --ignored --nocapture >"$gate_dir/gizmo-cpu-before-after.log" 2>&1; then tail -n 20 "$gate_dir/gizmo-cpu-before-after.log";
else tail -n 90 "$gate_dir/gizmo-cpu-before-after.log"; exit 1; fi