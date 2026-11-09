set -eu
cd /home/islam/ara-ux-editor
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=6
export CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_BUILD_DIR="$PWD/target"
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/x86_64"
export PATH="$VULKAN_SDK/bin:$PATH"
export LD_LIBRARY_PATH="$VULKAN_SDK/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export VK_LAYER_PATH="$VULKAN_SDK/share/vulkan/explicit_layer.d"
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 SDL_VIDEODRIVER=wayland
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/feedback-hub-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
git fetch /home/islam/ara.git t3/ux-editor-p4-feedback-native
for file in crates/ara-ui-test/examples/catalogue.stories crates/ara-ui-test/goldens/widget-feedback-[0-5].png crates/ara-ui-test/goldens/widget-banner-[0-5].png; do
 test "$(git hash-object "$file")" = "$(git rev-parse "FETCH_HEAD:$file")"
done
# Only generated, reviewed files identical to the committed tree enter the index.
# The ordinary checkout can then adopt that commit without overwriting dirty work.
git add -- crates/ara-ui-test/examples/catalogue.stories crates/ara-ui-test/goldens/widget-feedback-[0-5].png crates/ara-ui-test/goldens/widget-banner-[0-5].png
git checkout --detach FETCH_HEAD
test -z "$(git status --porcelain)"
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
run_case hub-feedback-tests cargo test -p ara-hub --locked
run_case hub-feedback-clippy cargo clippy -p ara-hub --all-targets --locked -- -D warnings
run_case hub-feedback-build cargo build -p ara-hub --locked
run_case hub-feedback-capture target/debug/ara-hub --capture "$gate_dir/hub-native" --size 1440x900 --dpi 1
printf 'FEEDBACK_HUB_GATE_COMPLETE %s\n' "$(git rev-parse HEAD)"
