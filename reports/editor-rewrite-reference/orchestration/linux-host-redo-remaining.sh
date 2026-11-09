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
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/redo-remaining-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
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
# The independent J7 ReflectionProbe rename has no compiled example yet. VQ owns
# the follow-up; keep its explicitly known docs dependency visible, not waived.
run_case host-assets-without-j7-docs cargo test -p ara-host -p ara-asset-host -p ara-cmd --locked -- --skip structured_reference_round_trips_and_covers_every_catalog_and_example
run_case strict-host cargo clippy -p ara-host -p ara-asset-host -p ara-cmd -p ara-editor --all-targets --locked -- -D warnings
run_case diag-coverage cargo test -p ara-diag --test coverage --locked
run_case telemetry-output cargo test -p ara-dev --bin ara-dev gpu::telemetry_tests --locked
run_case fmt cargo run -p ara-dev --locked -- fmt --check
printf 'HOST_REDO_REMAINING_COMPLETE %s\n' "$(git rev-parse HEAD)"
