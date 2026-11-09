set -eu
cd /home/islam/ara-ux-editor
test "$(git rev-parse --short HEAD)" = fc319865
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=6
export CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_BUILD_DIR="$PWD/target"
export DOTNET_CLI_HOME="$PWD/target/ux-p4/dotnet-home"
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/x86_64"
export PATH="$VULKAN_SDK/bin:$PATH"
export LD_LIBRARY_PATH="$VULKAN_SDK/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export VK_LAYER_PATH="$VULKAN_SDK/share/vulkan/explicit_layer.d"
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json
export XDG_RUNTIME_DIR=/run/user/1000 SDL_VIDEODRIVER=x11 ARA_PLAY_OFFSCREEN=1
while IFS='=' read -r env_name env_value; do
  case "$env_name" in DISPLAY|XAUTHORITY) export "$env_name=$env_value" ;; esac
done < <(systemctl --user show-environment | grep -e '^DISPLAY=' -e '^XAUTHORITY=')
functional_profile=$(mktemp -d "$PWD/target/ux-p4/profiles/functional-XXXXXX")
export ARA_CONFIG_DIR="$functional_profile/config" ARA_CRASH_DIR="$functional_profile/crashes"
gate_dir="$PWD/target/ux-p4/fc319865"
# Functional offscreen captures, not a timing/benchmark run. The Windows-only
# native process GPU monitor cannot admit this Linux node; retain that refusal.
capture_dir="$gate_dir/native-behaviours-x11"
test ! -e "$capture_dir"
mkdir -p "$capture_dir"
exec nice -n 10 target/debug/ara-editor "$capture_dir/unused" --verify-behaviours --output "$capture_dir" --size 1440x900 --dpi 1 >"$gate_dir/native-functional-x11.stdout" 2>"$gate_dir/native-functional-x11.stderr"
