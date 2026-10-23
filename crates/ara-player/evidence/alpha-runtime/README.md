# Alpha runtime evidence

Windows 11, RTX 4070 SUPER, Vulkan SDK 1.4.363.0, `CARGO_BUILD_JOBS=2`.
Baseline player: unmodified `5ba0e49`, saved in
`target/runtime-polish/ara-player-baseline.exe`.

| Measurement | Before | After |
| --- | ---: | ---: |
| Play to first C# tick, modules loaded | 4290.82 ms | 110.34 ms |
| Restart keeping C# and spawned-entity state | 6409.30 ms | 86.85 ms |
| Valley median of reported p95 windows | 5.2211 ms | 5.8219 ms |
| Valley highest reported p95 window | 5.7530 ms | 6.4356 ms |

C# also measured 58.72 ms in the earlier full player run. The final 110.34 ms
run overlapped other workspace builds. The test requires tick 1 in the Play
response and enforces the 500 ms limit. CoreCLR, Ara.Runtime, the compile server,
module compilation and loading run before pool hello, without running gameplay.

The frame comparison uses the same stationary camp camera at 1280x720, seed 9,
three seconds settling and seven seconds of 250 ms status samples. These are
statistics of the player's reported p95 windows, not one aggregate frame percentile.
The benchmark enforces a 16.67 ms budget. Raw samples are in
[before](valley-budget-before.json) and [after](valley-budget-after.json).

- [Valley before](valley-camp-before.png) and [Valley after](valley-camp-after.png):
  a running SDL player; `presented: true` reads the same GPU image as the swapchain.
  Foliage and campfire smoke are visible after composition.
- [Native 2D](2d-window.png): the platformer presented through ara-render2d,
  without capture readbacks in its frame loop.
- [Network host](net-host.png) and [client](net-client.png): host + one under
  40 ms latency, 5 ms jitter and 1% loss. Look turns the camera to 90 degrees;
  sprint moves forward along world +X; the client predicts a roughly 0.98 m jump,
  lands and converges within 2 cm. Final maximum correction was 23 mm.

C# Relevant health replicated as 65 on both avatars. Owner-only medkits changed
to 2 for player 1 while player 2 retained 1. Quantization, nested privacy,
rollback and adverse-link ara-net gates also passed. Field policies and the
mirror component catalog are compiled at connection: schema-changing C# reloads
require restarting the network session; live policy renegotiation is not present.

Missing model sources produce ARA7303 and magenta placeholder geometry while
simulation continues. Missing atlases retain sprite, flipbook and tile regions
in a checkerboard placeholder. Missing Vulkan/.NET probes verify coded errors
with `ara doctor` hints. The first window frame announces bounded GPU warm-up
grace and explicitly restores the normal watchdog afterward. A real native
window close stopped and reaped the world cleanly.

Panic and native access-violation probes produced minidumps. The retained pair is
under `target/runtime-polish/crashes/final/`: `access-2272-1792774070487791000.dmp`,
its crash JSON, the matching `ara-player.exe` and `ara_player.pdb`.
`symbolized.txt` records CDB resolving the native stack through
`ara_player::crash::trigger`. Build ID:
`ara-player-2e2a9eb5aae08aeba4038092eb425330ee297ab4e176438eccfe92706f9c01c8`.

Validation logs are under `target/runtime-polish/`:

- `final-check.log`, `final-clippy.log`, `player-final-clippy.log`: touched-crate
  check and all-target clippy with warnings denied; scoped rustfmt check passed.
- `crate-tests.log`: runtime, render, scripts, runtime-net, ara-net, ara-sim,
  play-host, render2d and landscape tests, including unchanged 2D/landscape goldens.
- `final-player-tests.log`: all ten default player tests pass, including scenario,
  mixed, Valley, pins, C#, network, 2D and missing prerequisites.
- `final-window-tests.log`: all five explicit native window tests pass, including
  the moving Valley crossing into c_10 and the stationary frame budget.

The ara-render2d APIs are additive; its existing headless and capture APIs remain.
The only ara-landscape change adds `SAMPLED` usage to its color output.
`ara-dev` was absent from fetched main `3fff9418`, so its conditional check did not apply.
