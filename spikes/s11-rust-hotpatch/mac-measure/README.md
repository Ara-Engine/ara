# Manual macOS measurement controller

Uses the existing .NET 10 SDK, with no package dependencies. This is an explicit
source-edit experiment, never a Cargo test. Use a dedicated checkout with a clean
copy of the selected source. The controller restores the original body in `finally`.

Check `pmset -g batt` for AC before each job. Build the controller once, outside
any other Mac build, with scratch paths:

```sh
export DOTNET_CLI_HOME="$PWD/target/hotpatch-evidence/dotnet-home"
export DOTNET_GENERATE_ASPNET_CERTIFICATE=false DOTNET_CLI_TELEMETRY_OPTOUT=1
pmset -g batt | grep 'AC Power'
caffeinate -dimsu dotnet build spikes/s11-rust-hotpatch/mac-measure \
  --artifacts-path "$PWD/target/hotpatch-evidence/dotnet" \
  -o "$PWD/target/hotpatch-evidence/controller"
```

Launch `ara-dev hotpatch -- sim-probe --output <output>` or the offscreen editor
lane on a scratch project. Set `ARA_HOTPATCH_TRACE_DIR=<output>/trace` on the
driver before launching. Keep its build target in the owned checkout, and run
only one Rust build at a time. While that driver runs:

```sh
dotnet target/hotpatch-evidence/controller/mac-measure.dll sim "$PWD" <output>
dotnet target/hotpatch-evidence/controller/mac-measure.dll editor "$PWD" <output> <scratch-project>
```

Run only the matching command. Each makes four alternating body edits, checks
the same PID and actual system output or captured panel, saves `latency.json`,
then asks the host to exit. The editor also saves real PNGs through its production
protocol. Stop the exact owned DX server after the host exits. Copy evidence off
the Mac before cleaning its build-only target directories.
