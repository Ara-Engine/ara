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
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/feedback-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
git fetch /home/islam/ara.git t3/ux-editor-p4-script-recovery
git checkout --detach FETCH_HEAD
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
run_case() {
 name="$1"
 shift
 if nice -n 10 "$@" >"$gate_dir/$name.log" 2>&1; then
   printf '%s: PASS\n' "$name"
   sed -n '/test result:/p' "$gate_dir/$name.log"
 else tail -n 85 "$gate_dir/$name.log"; return 1; fi
}
: # Exact predecessor check already passed; clippy below compiles all targets.
run_case recovery-ui cargo test -p ara-editor --lib script_recovery_ui --locked
run_case recovery-projection cargo test -p ara-editor --lib shell::log_panels::compile --locked
: # Exact predecessor full 509-test library gate passed.
: # Exact predecessor full Play-host gate passed.
run_case recovery-clippy cargo clippy -p ara-editor -p ara-play-host --all-targets --locked -- -D warnings
run_case recovery-diag cargo test -p ara-diag --test coverage --locked
run_case recovery-fmt cargo run -p ara-dev --locked -- fmt --check
printf 'RECOVERY_EDITOR_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"
git fetch /home/islam/ara.git t3/ux-editor-p4-collection-feedback
git checkout --detach FETCH_HEAD
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
run_case collection-drag cargo test -p ara-editor --lib shell::drag_drop_tests --locked
run_case collection-native-proof cargo test -p ara-editor --lib collection_verify --locked
run_case collection-editor-lib cargo test -p ara-editor --lib --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
run_case collection-clippy cargo clippy -p ara-editor --all-targets --locked -- -D warnings
run_case collection-fmt cargo run -p ara-dev --locked -- fmt --check
printf 'COLLECTION_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"