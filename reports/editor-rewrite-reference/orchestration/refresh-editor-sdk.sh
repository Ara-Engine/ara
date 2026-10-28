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
run_case sdk-files-before bash -c 'for n in Ara.CompileServer Ara.Runtime Ara.Generators Ara.Analyzers Ara.Editor; do if test -f dotnet/artifacts/bin/Ara.CompileServer/release/$n.dll; then printf "%s present\n" "$n"; else printf "%s missing\n" "$n"; fi; done'
run_case dotnet-build dotnet build dotnet/Ara.slnx -c Release --nologo --disable-build-servers -m:6
run_case shipping-scripts cargo test -p ara-export --test shipping_scripts --locked
run_case monkey-instrumentation cargo test -p ara-editor --lib app::monkey --locked
run_case panel-cpu cargo test -p ara-editor --lib panel_compilation_cache_cpu_benchmark --locked -- --ignored --nocapture
run_case diagnostics cargo test -p ara-diag --test coverage --locked