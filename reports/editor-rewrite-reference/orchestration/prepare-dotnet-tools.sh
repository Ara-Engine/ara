set -eu
cd /home/islam/ara-ux-editor
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" DOTNET_CLI_HOME="$PWD/target/ux-p4/dotnet-home"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_GENERATE_ASPNET_CERTIFICATE=false
gate_dir="target/ux-p4/$(git rev-parse --short HEAD)"
mkdir -p "$gate_dir"
if nice -n 10 dotnet build dotnet/Ara.slnx -c Release --disable-build-servers -m:3 >"$gate_dir/dotnet-tools.log" 2>&1; then tail -n 20 "$gate_dir/dotnet-tools.log";
else tail -n 100 "$gate_dir/dotnet-tools.log"; exit 1; fi
ASSET_RESUME_TESTS=1 bash target/ux-p4/asset-slice-gates.sh