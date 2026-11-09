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
brand_profile=$(mktemp -d "$PWD/target/ux-p4/profiles/brand-XXXXXX")
export ARA_CONFIG_DIR="$brand_profile/config" ARA_CRASH_DIR="$brand_profile/crashes"
export ARA_PLAY_OFFSCREEN=1
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
nice -n 10 cargo run -p ara-dev --locked -- cargo -- build -p ara-editor --locked >"$gate_dir/paired-build.log" 2>&1 || { tail -n 80 "$gate_dir/paired-build.log"; exit 1; }
tail -n 15 "$gate_dir/paired-build.log"
capture_dir="$PWD/$gate_dir/native-behaviours"
test ! -e "$capture_dir"
mkdir -p "$capture_dir"
nice -n 10 target/debug/ara-dev gpu --timeout 300 -- target/debug/ara-editor "$capture_dir/unused" --verify-behaviours --output "$capture_dir" --size 1440x900 --dpi 1 >"$gate_dir/native-behaviours.stdout" 2>"$gate_dir/native-behaviours.stderr"
