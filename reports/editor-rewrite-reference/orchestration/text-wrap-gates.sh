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
run_case() {
 name="$1"
 shift
 mkdir -p target/ux-p4/text-wrap-gates
 if nice -n 10 "$@" >"target/ux-p4/text-wrap-gates/$name.log" 2>&1; then tail -n 12 "target/ux-p4/text-wrap-gates/$name.log";
 else tail -n 80 "target/ux-p4/text-wrap-gates/$name.log"; return 1; fi
}
git fetch /home/islam/ara.git t3/ux-editor-p4-gestures-ready t3/ux-editor-p4-text-baseline t3/ux-editor-p4-text-ready
git checkout --detach 0755a5ab
run_case import-layout cargo test -p ara-editor --lib bounded_import_rail_exposes_cancel_and_dismiss_without_collapsing_the_browser --locked
git checkout --detach 05380ce6
export ARA_EVIDENCE_SOURCE=05380ce6
run_case narrow-before cargo run -p ara-ui-test --example select_narrow_native --locked -- target/ux-p4/select-narrow/before
git checkout --detach a5f0c4f3
export ARA_EVIDENCE_SOURCE=a5f0c4f3
run_case narrow-after cargo run -p ara-ui-test --example select_narrow_native --locked -- target/ux-p4/select-narrow/after
run_case fmt cargo run -p ara-dev --locked -- fmt --check
run_case clippy cargo clippy -p ara-ui-text -p ara-ui -p ara-ui-document -p ara-ui-test -p ara-ui-stories -p ara-editor --all-targets --locked -- -D warnings
run_case kit-tests cargo test -p ara-ui-text -p ara-ui -p ara-ui-document -p ara-ui-test -p ara-ui-stories --locked
run_case editor-lib cargo test -p ara-editor --lib --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
run_case editor-focused cargo test -p ara-editor --test ux --test script_assets --locked
run_case diagnostics cargo test -p ara-diag --test coverage --locked