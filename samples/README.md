# Ara samples

Real-art test projects, authored through Ara's own commands by `ara-samples`
(`crates/ara-samples`). No third-party binary is in git: `assets.lock.json` pins every
file (URL, BLAKE3, size, license, authors, source page) and `fetch` restores them.

| Project | Scenes |
|---|---|
| `outpost/` | `game/levels/outpost.scene`: Outpost at dusk (a derelict compound on 384 m of terrain with foliage, a campfire, floodlights, wind and music). `game/levels/hangar.scene`: the hangar interior at night, lit by emissive panels, a festoon and a few lamps. |
| `showcase/` | `game/levels/showcase.scene`: a turntable stage with an animated mannequin, a jogger, 11 hero props on plinths and 18 material balls under a studio HDRI. |

## How to play Outpost

Recover eight glowing signal caches, then bring every active scavenger to the blue
extraction beacon for three seconds. Nightfall ends the run after five minutes.
Security drones chase and damage you; **E** stuns a nearby drone. Hold **E** beside a
downed teammate to revive them. The crew has three emergency rescues.

**WASD** moves, **Shift** sprints, **Space** jumps, **right mouse drag** orbits the
camera, **E** opens a shared hatch or takes salvage, **Tab** opens your private
satchel, and **Escape** opens the pause menu. Settings includes graphics quality,
resolution, window mode, volume, mouse sensitivity/invert and key bindings.

From a Windows checkout, prepare the assets and tools once (PowerShell):

```powershell
$env:CARGO_BUILD_JOBS = '3'
$env:CARGO_TARGET_DIR = 'C:\AraBuild\outpost-game-target'
.\crates\ara-dev\run.ps1 cargo -- -- build -j 3 --config tests/samples/outpost/windows-build.toml -p ara -p ara-player -p ara-import-worker -p ara-samples -p ara-export -p ara-shader-cook
$bin = Join-Path $env:CARGO_TARGET_DIR 'debug'
& "$bin\ara-samples.exe" fetch outpost
& "$bin\ara-samples.exe" verify outpost
& "$bin\ara-samples.exe" kits outpost
dotnet build dotnet/Ara.slnx -c Release
# Repeat only when deliberately reauthoring the sample:
& "$bin\ara-samples.exe" game
& "$bin\ara-player.exe" --game --project samples/outpost
```

Choose **Play solo**, or **Host a crew** and then **Ready**. On the same PC launch
another player with the last command, enter `127.0.0.1:41090` in **Join**, and ready
up. Up to three clients can join the listen host, including after a hatch or cache
has changed. For a dedicated authority and up to four clients:

```powershell
& "$bin\ara-player.exe" --game --project samples/outpost --dedicated --headless --port 41090
& "$bin\ara-player.exe" --game --project samples/outpost --connect 127.0.0.1:41090
```

In the editor, open `samples/outpost`, select `game/levels/outpost.scene`, and choose
**Host + N** or **Dedicated + N** in Play's multiplayer configuration. Each view uses
its own input owner and native HUD. Source launches cook missing assets on the
loading screen; an exported build loads its pinned cooked pack.

To export a playable Windows folder with NativeAOT gameplay:

```powershell
.\tests\samples\outpost\export.ps1 -Bin $bin -Output C:\AraBuild\Outpost-Windows
```

Open `C:\AraBuild\Outpost-Windows\build\Outpost.exe` to reach the main menu. Run it a second time and use
**Join** to play co-op on this PC. For dedicated play, run `Outpost.exe --game
--dedicated --headless`, then open client instances normally. `--windowed` overrides
the saved startup window mode. Leave the existing output folder intact; choose a
new output path for another export.

For two visible windows on one PC, launch from the exported folder twice:

```powershell
Start-Process .\Outpost.exe -ArgumentList '--windowed'
Start-Process .\Outpost.exe -ArgumentList '--windowed'
```

Choose **Host a crew** in the first, then **Join** `127.0.0.1:41090` in the second.
Ready each scavenger, then switch windows with Alt+Tab to control that player.

The offscreen gate uses fresh profiles and only its own child processes:

```powershell
.\tests\samples\outpost\coop.ps1 -Bin $bin
```

It tests solo, then drives four players in listen and dedicated modes, joins the last player after a
shared hatch opens, checks private inventories, captures each client's HUD/satchel/
door/result view, and verifies two authority replays against live checkpoints.
The scout tests real drone damage and the crew must revive it; every capture is 1080p.
Live recordings hash tick multiples of 30 and the final tick; offline replays hash
every tick. Normal play leaves state hashing off. To test the exported build, add
`-ExportRoot C:\AraBuild\Outpost-Windows\build`. Art, fonts and sounds are
listed in [THIRD_PARTY_ASSETS.md](THIRD_PARTY_ASSETS.md).

```sh
cargo build --release -p ara -p ara-player -p ara-import-worker -p ara-editor -p ara-samples
target/release/ara-samples fetch                        # ~510 MB, resumable, hash-checked
target/release/ara-samples build outpost                # and: build showcase
target/release/ara-samples capture outpost outpost      # looks: outpost, hangar; showcase showcase
target/release/ara-samples play outpost levels/hangar.scene hangar
target/release/ara-samples editor showcase levels/showcase.scene showcase
```

Builds use the same document, import, animation, music and scene commands as CLI/MCP
agents. Variant sheets become native prefabs with source-node selectors; lamp
emission and suit materials are component overrides. The original Poly Haven fence
uses its texture transforms and companion opacity maps during import. Terrain uses
its authored signed origin, with native material graphs scaled in world metres.
Colliders share the prop entity and automatic physics identities. Rebuilds preserve
the attributed journal and keep stable document, stroke and exclusion IDs.

The projects are ordinary Ara projects: `ara check`, `ara mcp` and the editor work in
them as in any other. GPU commands need the Vulkan SDK (see `crates/ara-samples/AGENTS.md`).

- `<project>/game/look/*.look.json`: what a scene cannot express yet for the captures:
  camera shots, an HDRI sky and the terrain ground material, plus temporary tuning
  overrides. Physical sky, height fog and exposure/post live in the scenes
  (`SkyAtmosphere`, `HeightFog`, `PostProcess`).
- `../evidence/samples/<project>/`: checked-in reference captures. Generated
  fidelity, Play and editor captures go under `target/ara-samples/`, outside the
  game project. `ARA_CAPTURE_OUTPUT_DIR` explicitly selects another destination.
- `settings/builds/windows.export.json`: the game's export settings. The repo's
  `tests/samples/outpost/export.ps1` helper supplies a local runtime pin; projects
  no longer carry machine-specific export scripts or Cargo profile files.
- [Project layout and migration](../docs/project-layout.md): creation, references,
  ignored output roots, and `ara project migrate --dry-run`.
- [`FINDINGS.md`](FINDINGS.md): engine bugs and friction found building these, with
  repros. [`THIRD_PARTY_ASSETS.md`](THIRD_PARTY_ASSETS.md): attribution.
