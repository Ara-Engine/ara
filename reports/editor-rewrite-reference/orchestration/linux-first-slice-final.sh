set -eu
cd /home/islam/ara-ux-editor
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=6
export CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_BUILD_DIR="$PWD/target"
export DOTNET_CLI_HOME="$PWD/target/ux-p4/dotnet-home"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_GENERATE_ASPNET_CERTIFICATE=false
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/x86_64"
export PATH="$VULKAN_SDK/bin:$PATH"
export LD_LIBRARY_PATH="$VULKAN_SDK/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export VK_LAYER_PATH="$VULKAN_SDK/share/vulkan/explicit_layer.d"
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 SDL_VIDEODRIVER=wayland
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/closure-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
git fetch /home/islam/ara.git t3/ux-editor-p4
git checkout --detach FETCH_HEAD
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
run_case() {
 name="$1"
 shift
 if nice -n 10 "$@" >"$gate_dir/$name.log" 2>&1; then
   printf '%s: PASS\n' "$name"
   sed -n '/test result:/p' "$gate_dir/$name.log"
 else tail -n 75 "$gate_dir/$name.log"; return 1; fi
}
run_case touched-clippy cargo clippy -p ara-play-host -p ara-editor -p ara-platform -p ara-host -p ara-hub -p ara-ui -p ara-ui-test --all-targets --locked -- -D warnings
run_case touched-fmt cargo run -p ara-dev --locked -- fmt --check
run_case narrow-lib cargo test -p ara-doc -p ara-dev -p ara-net -p ara-runtime-net -p ara-export -p ara-shader-cook --lib --locked
run_case layout-guards cargo test -p ara-dev --test project_layout --locked
run_case narrow-clippy cargo clippy -p ara-doc -p ara-dev -p ara-net -p ara-runtime-net -p ara-export -p ara-shader-cook --all-targets --locked -- -D warnings
run_case compile-poll-cpu env ARA_EDITOR_BENCH_COMPILE_POLL=1 cargo test -p ara-editor --lib shell::log_panels::compile::tests::poll::guarded_poll_cpu_benchmark_against_old_query_and_scalar_revision_check --locked -- --ignored --exact --nocapture
run_case diagnostic-cpu cargo test -p ara-editor --lib diagnostic_cache_measurement --locked -- --ignored --nocapture
run_case pilot-cpu cargo test -p ara-editor --lib pilot --locked -- --ignored --nocapture
printf 'FOCUSED_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"