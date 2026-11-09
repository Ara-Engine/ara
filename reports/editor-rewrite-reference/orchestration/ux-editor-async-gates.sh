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
profile=$(mktemp -d "$PWD/target/ux-p4/profiles/async-XXXXXX")
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
 else tail -n 90 "$gate_dir/$name.log"; return 1; fi
}
run_case async-play-focused cargo test -p ara-play-host --lib starting:: --locked
run_case async-script-inventory cargo test -p ara-editor --lib shell::scripts::module_discovery --locked
run_case policy-invalid cargo test -p ara-editor --lib malformed_policy_after_open --locked
run_case drag-content cargo test -p ara-editor --lib shell::drag::content::tests:: --locked
run_case native-row-sizing cargo test -p ara-editor --lib narrow_inspector_and_import_selects_fit_auto_rows_without_expanding_numeric_density --locked
run_case monkey-tests cargo test -p ara-editor --lib app::monkey:: --locked
run_case divergence-accessor cargo test -p ara-host --lib document_diverged_distinguishes --locked
run_case fixture-guard cargo test -p ara-dev --test project_layout --locked
run_case legacy-menu cargo test -p ara-editor --lib legacy_projects_are_rejected_without_mutating_prefab_menu_fixtures --locked
run_case legacy-panels cargo test -p ara-editor --lib panel_fixtures_reject_legacy_projects_without_writing --locked
run_case legacy-policy cargo test -p ara-editor --lib live_projection_obeys_empty_null_invalid_and_legacy_sections_without_writing_files_or_history --locked
run_case play-start-ui cargo test -p ara-editor --test play_start --locked
run_case play-host-suite cargo test -p ara-play-host --locked
run_case host-suite cargo test -p ara-host --locked
run_case editor-tests cargo test -p ara-editor --tests --locked -- --skip scene_captures_native_quality_environment_and_effects_after_resize
run_case cli-reflection cargo test -p ara --test cli --locked
run_case touched-clippy cargo clippy -p ara-play-host -p ara-editor -p ara-platform -p ara-host --all-targets --locked -- -D warnings
run_case touched-fmt cargo run -p ara-dev --locked -- fmt --check
