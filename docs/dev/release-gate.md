# Final release gate

Run on the release commit, after its batch gates, with no competing performance
measurements. `CARGO_BUILD_JOBS=4` is honoured after `13f6cb68`; an unset value
defaults to two. Agent checkpoints continue to set two explicitly.

## What `check --full` adds

- Formatting and strict `clippy --workspace --all-targets` for every package,
  rather than the changed packages' reverse dependency closure.
- One full-workspace test-profile build of libraries, tests, regular executables
  and examples, with a common feature union and exact peer executable paths.
- Every workspace non-ignored harness, plus workspace doctests. Batch execution
  stops at source owners/direct consumers; full includes every transitive consumer
  and unrelated package. Slow-tier rules apply in both modes.
- No overall 40-minute deadline by default. Explicit `--timeout N` still applies;
  `--timeout 0` is unlimited. Per-suite deadlines remain: fast 120 s, slow
  180/240 s, material-library 300 s, self-building ara-dev commands 600 s.

The same pin/AGENTS/importer-version/catalog/clean-tree/snapshot checks apply.
Full does **not** pass `--ignored`, `--include-ignored`, `--all-features` or run
benchmarks. Optional feature gates and explicit hardware/export checks remain
separate. Do not use blanket `--include-ignored`: several ignored entries are
child-process fixtures that their ordinary parent tests already launch.

At the audit's current main `0614a7f2`, batch 41 selected ara-editor and
ara-importers. Against the same-day complete workspace inventory, full adds
**102 packages / 355 harnesses**, including **102 slow-tier harnesses**. These
counts depend on the batch; a batch already selecting the whole workspace has
no additional suite coverage. The complete inventory is 104 packages,
374 harnesses, including 121 slow-tier harnesses.

## Duration on this PC

Same-day green full-union evidence:
`target/ara-dev/run-70828-1793019231405/` in the integrator checkout. All 374
harnesses passed. The two-job phases were clippy **89.8 s**, test build **932.1 s**,
harnesses **445.9 s**, doctests **33.8 s**, and approximately 26 s of metadata,
formatting, hygiene and process transitions: **25m28s total**.

At four build jobs, estimate **18-24 minutes** with a similarly warm dependency
cache; reserve 30 minutes. This is an estimate, not a measured four-job run.
Planning split: about 1 minute of clippy, 9-14 minutes of test compilation,
7-8 minutes of harness execution and 1 minute for the other phases.
Only compilation gains concurrency: harness admission still defaults to two
workers, exclusive GPU/.NET/process groups, at most 4096 MiB and half available
RAM. Shared SATA I/O, low RAM or cold dependencies can exceed this estimate.
The all-dependency cold-build case is not represented by this measurement.

No duplicate full-union build or additional suite run was started in the audit
worktree: the integrator explicitly retained that work for the final release
gate to protect disk space and renderer builds. No new full-suite failure or
flake result is claimed. The separate concurrency fix passes 48 unit tests,
19 command tests and two GPU-queue tests, strict Clippy and workspace fmt.

## Curated ignored checks

Run these after the final full gate. Reuse its artifact paths to avoid another
feature resolution/build. Vulkan tests need the SDK/validation layers and GPU;
the Play sharing test needs Win32 external memory. Keep golden regeneration
disabled. Everything below uses isolated checkout profiles and one GPU lease
at a time, including export checks that launch the player.

In PowerShell, set `$Gate` to the exact **completed final full gate** log folder:

```powershell
$Gate = 'D:/Coding/projects/desktop/snengine/target/ara-dev/run-<final-full-gate>'
$Workspace = (git rev-parse --show-toplevel).Trim()
$Meta = Get-Content "$Gate/metadata.json" -Raw | ConvertFrom-Json
$Artifacts = @(Get-Content "$Gate/test-build.json" | ForEach-Object {
    $_ | ConvertFrom-Json
} | Where-Object reason -eq 'compiler-artifact')
$Peers = @($Artifacts | Where-Object {
    $_.executable -and -not $_.profile.test -and $_.target.kind -contains 'bin'
})
foreach ($peer in $Peers) {
    Set-Item "Env:CARGO_BIN_EXE_$($peer.target.name)" $peer.executable
}
$Dev = ($Peers | Where-Object { $_.target.name -eq 'ara-dev' } | Select-Object -First 1).executable
$env:ARA_PLAYER = ($Peers | Where-Object { $_.target.name -eq 'ara-player' } | Select-Object -First 1).executable
$env:ARA_IMPORT_WORKER = ($Peers | Where-Object { $_.target.name -eq 'ara-import-worker' } | Select-Object -First 1).executable
$env:ARA_TEST_IMPORT_WORKER = $env:ARA_IMPORT_WORKER
function TestExe($package, $target) {
    $manifest = ($Meta.packages | Where-Object name -eq $package).manifest_path
    $found = @($Artifacts | Where-Object {
        $_.manifest_path -eq $manifest -and $_.target.name -eq $target -and
        $_.profile.test -and $_.executable
    } | ForEach-Object executable | Sort-Object -Unique)
    if ($found.Count -ne 1 -or -not (Test-Path -LiteralPath $found[0])) { throw "Missing unique $package/$target harness" }
    $found[0]
}
function CheckIgnored($package, $target, $test) {
    $env:ARA_CONFIG_DIR = "$Workspace/target/release-preflight/$package-$target/profile"
    $env:ARA_CRASH_DIR = "$Workspace/target/release-preflight/$package-$target/crashes"
    & $Dev gpu -- (TestExe $package $target) --ignored --exact $test --test-threads=1
    if ($LASTEXITCODE) { throw "$package/$target/$test failed: $LASTEXITCODE" }
}
Remove-Item Env:ARA_EDITOR_UPDATE_GOLDENS -ErrorAction SilentlyContinue
```

The short shipping shortlist:

```powershell
# Integer GPU codec correctness, all formats/presets, scalar and optional dot paths.
CheckIgnored ara-texture-gpu ara_texture_gpu 'tests::unified_host_matches_integer_oracles_and_reuses_transfers'
# Real worker framing, backend identity, pooled device reuse and no silent fallback.
CheckIgnored ara-import-worker gpu 'gpu_worker_negotiates_and_encodes_without_contaminating_protocol_stdout'
# Cross-process embedded Play surface, rendered by the owner device.
CheckIgnored ara-player embed 'embedded_play_shares_frames_with_the_owner_device'
# Hidden first-loading-frame presentation and GPU handoff while warmup blocks.
CheckIgnored ara-player ara-player 'standalone::startup::tests::loading_can_present_while_presenter_warmup_is_blocked_and_then_hands_off_its_gpu'
# Offscreen native UI interactions and reviewed dark/light screenshots; never auto-update.
CheckIgnored ara-editor native 'native_interaction_route_matches_reviewed_dark_and_light_goldens'
```

Two additional shipping checks require staged artifacts. Use the actual shipped
player and its **matching** PDB for the first, and the built Hub/updater/setup
directory for the second. The tiny installation smoke uses its own profiles;
it does not replace the user's installed engine.

```powershell
# Set ARA_SHIPPING_E2E_PLAYER and ARA_SHIPPING_E2E_PDB to the shipping pair first.
if (-not $env:ARA_SHIPPING_E2E_PLAYER -or -not $env:ARA_SHIPPING_E2E_PDB) { throw 'Set shipping player/PDB paths' }
CheckIgnored ara-export runtime_csharp 'exported_game_ticks_csharp_after_relocation_without_author_files'
$env:ARA_TEST_BIN_DIR = Split-Path $Dev
CheckIgnored ara-release ara-release 'smoke::hub_upgrade::tests::installed_packages_replace_a_stale_hub_and_launch_new_build_through_shortcuts'
```

If the distribution includes Steam Audio, also qualify its supplied runtime:
`CheckIgnored ara-audio native 'measured_hrtf_has_direction_occlusion_and_a_repeatable_render'`
and `CheckIgnored ara-acoustics acoustics 'steam_audio_bakes_parametric_reverb'`.
Set `ARA_STEAM_AUDIO_LIBRARY` and `ARA_STEAM_AUDIO` to that pinned `phonon.dll`
when it is not at the checkout's documented runtime path.

Leave Android/cross-platform distribution matrices to their prepared SDK/pin
lanes. Leave timing probes, million-triangle experiments and private subprocess
helpers out of a generic ignored-test sweep. The D26 scoreboard remains a
separate acceptance gate; none of these smoke tests is a timing acceptance claim.
