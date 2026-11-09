set -u
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
 else tail -n 45 "$gate_dir/$name.log"; gate_failed=1; fi
}
gate_failed=0
run_case host-job-controls cargo test -p ara-host --lib job_control_tests --locked
run_case play-host cargo test -p ara-play-host --locked
run_case kit cargo test -p ara-ui -p ara-ui-test --locked
run_case diag-coverage cargo test -p ara-diag --test coverage --locked
run_case hub-build cargo build -p ara-hub --locked
run_case hub-offscreen target/debug/ara-hub --capture "$gate_dir/hub-capture" --size 1440x900 --dpi 1
run_case cli-reflection cargo test -p ara --test cli --locked
run_case touched-clippy cargo clippy -p ara-play-host -p ara-editor -p ara-platform -p ara-host -p ara-hub -p ara-ui -p ara-ui-test --all-targets --locked -- -D warnings
run_case touched-fmt cargo run -p ara-dev --locked -- fmt --check
printf 'REMAINING_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"
exit "$gate_failed"
