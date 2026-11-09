set -eu
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
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/run-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
failed=0
nice -n 10 cargo test -p ara-editor --lib retained_main_tooltips_fit_their_final_shaped_text --locked -- --nocapture >"$gate_dir/tooltip-focused.log" 2>&1 || failed=1
tail -n 32 "$gate_dir/tooltip-focused.log"
nice -n 10 cargo clippy -p ara-ui -p ara-doc -p ara-cmd -p ara-platform -p ara-play -p ara-play-host -p ara-runtime-net -p ara-runtime-render -p ara-render -p ara-vfx-gpu -p ara-player -p ara-editor --all-targets --locked -- -D warnings >"$gate_dir/clippy.log" 2>&1 || { tail -n 85 "$gate_dir/clippy.log"; exit 1; }
tail -n 4 "$gate_dir/clippy.log"
nice -n 10 cargo test -p ara-ui -p ara-play -p ara-play-host -p ara-runtime-net --lib --locked >"$gate_dir/control-libs.log" 2>&1 || { tail -n 100 "$gate_dir/control-libs.log"; exit 1; }
tail -n 15 "$gate_dir/control-libs.log"
nice -n 10 cargo test -p ara-editor --lib --locked -- --skip editor_overlays_and_control_choices_fit_all_mounted_contexts --skip scene_captures_native_quality_environment_and_effects_after_resize >"$gate_dir/editor-libs.log" 2>&1 || { tail -n 100 "$gate_dir/editor-libs.log"; exit 1; }
tail -n 16 "$gate_dir/editor-libs.log"
if [ "$failed" = 0 ]; then nice -n 10 cargo test -p ara-editor --lib editor_overlays_and_control_choices_fit_all_mounted_contexts --locked >"$gate_dir/full-overlays.log" 2>&1 || { tail -n 90 "$gate_dir/full-overlays.log"; exit 1; }; tail -n 8 "$gate_dir/full-overlays.log"; fi
exit "$failed"

