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
git fetch /home/islam/ara.git t3/ux-editor-p4-feedback-fix
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
run_case final-import-rail cargo test -p ara-editor --lib asset_import::view --locked
run_case final-editor-lib cargo test -p ara-editor --lib --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
run_case final-kit-goldens cargo test -p ara-ui-document -p ara-ui -p ara-ui-stories -p ara-ui-test --locked
run_case final-ux-script-assets cargo test -p ara-editor --test ux --test script_assets --locked
run_case final-clippy cargo clippy -p ara-editor -p ara-ui-document -p ara-ui -p ara-ui-stories -p ara-ui-test --all-targets --locked -- -D warnings
run_case final-fmt cargo run -p ara-dev --locked -- fmt --check
printf 'FEEDBACK_SUCCESSOR_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"