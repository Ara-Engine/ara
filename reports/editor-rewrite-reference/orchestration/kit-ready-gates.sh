set -eu
cd /home/islam/ara-ux-editor
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=6
export CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_BUILD_DIR="$PWD/target"
export DOTNET_CLI_HOME="$PWD/target/ux-p4/dotnet-home"
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/x86_64"
export PATH="$VULKAN_SDK/bin:$PATH"
export LD_LIBRARY_PATH="$VULKAN_SDK/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export VK_LAYER_PATH="$VULKAN_SDK/share/vulkan/explicit_layer.d"
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 SDL_VIDEODRIVER=wayland
kit_profile=$(mktemp -d "$PWD/target/ux-p4/profiles/kit-XXXXXX")
export ARA_CONFIG_DIR="$kit_profile/config" ARA_CRASH_DIR="$kit_profile/crashes"
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
run_gate() {
  gate_name="$1"
  shift
  if nice -n 10 "$@" >"$gate_dir/$gate_name.log" 2>&1; then
    tail -n 16 "$gate_dir/$gate_name.log"
  else
    tail -n 100 "$gate_dir/$gate_name.log"
    return 1
  fi
}
run_gate fmt cargo run -p ara-dev --locked -- fmt --check
run_gate clippy cargo clippy -p ara-ui -p ara-ui-document -p ara-ui-test -p ara-ui-stories -p ara-editor --all-targets --locked -- -D warnings
run_gate kit-full cargo test -p ara-ui -p ara-ui-document -p ara-ui-test -p ara-ui-stories --locked
run_gate editor-lib cargo test -p ara-editor --lib --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
run_gate diagnostics cargo test -p ara-diag --test coverage --locked
git status --porcelain >"$gate_dir/tree-after.txt"
test ! -s "$gate_dir/tree-after.txt"
printf 'KIT_READY_GATES_PASS %s\n' "$(git rev-parse HEAD)"
