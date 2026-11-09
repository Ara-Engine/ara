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
select_profile=$(mktemp -d "$PWD/target/ux-p4/profiles/select-XXXXXX")
export ARA_CONFIG_DIR="$select_profile/config" ARA_CRASH_DIR="$select_profile/crashes"
mkdir -p target/ux-p4/select-visual
nice -n 10 cargo run -p ara-ui-test --example select_capture -- "$PWD/target/ux-p4/select-visual/before" >target/ux-p4/select-visual/before.log 2>&1 || { tail -n 90 target/ux-p4/select-visual/before.log; exit 1; }
tail -n 12 target/ux-p4/select-visual/before.log
