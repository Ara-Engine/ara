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
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/assets-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
git fetch /home/islam/ara.git t3/ux-editor-p4
git checkout --detach FETCH_HEAD
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
run_case() {
 name="$1"
 shift
 if nice -n 10 "$@" >"$gate_dir/$name.log" 2>&1; then tail -n 14 "$gate_dir/$name.log";
 else tail -n 80 "$gate_dir/$name.log"; return 1; fi
}
run_case divergence-service-test cargo test -p ara-host --lib service::divergence_tests --locked
run_case typed-project-settings-tests cargo test -p ara-editor --lib settings:: --locked
run_case changed-clippy cargo clippy -p ara-doc -p ara-host -p ara-ui -p ara-ui-test -p ara-ui-stories -p ara-editor --all-targets --locked -- -D warnings
run_case changed-fmt cargo run -p ara-dev --locked -- fmt --check
run_case host-document-tests cargo test -p ara-doc -p ara-host --locked
run_case kit-tests cargo test -p ara-ui -p ara-ui-test -p ara-ui-stories --locked
run_case editor-lib cargo test -p ara-editor --lib --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
set -- cargo test -p ara-editor --locked
for integration in crates/ara-editor/tests/*.rs; do name=${integration##*/}; name=${name%.rs}; set -- "$@" --test "$name"; done
run_case editor-integration "$@"
run_case diagnostic-coverage cargo test -p ara-diag --test coverage --locked
