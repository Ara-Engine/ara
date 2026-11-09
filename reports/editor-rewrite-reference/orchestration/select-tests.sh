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
nice -n 10 cargo test -p ara-ui --lib select::tests --locked >"$gate_dir/popover-tests.log" 2>&1 || { tail -n 100 "$gate_dir/popover-tests.log"; exit 1; }
tail -n 5 "$gate_dir/popover-tests.log"
nice -n 10 cargo test -p ara-editor --lib settings_select_commits_mouse_keyboard_and_numeric_text_through_typed_actions --locked -- --nocapture >"$gate_dir/select-focused.log" 2>&1 || { tail -n 110 "$gate_dir/select-focused.log"; exit 1; }
tail -n 10 "$gate_dir/select-focused.log"