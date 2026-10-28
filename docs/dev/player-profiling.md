# Private player profiling

The native editor takes `PlayService::profiling()` before registering the service.
Its owned `PlayProfilingHandle` exposes the agreed `refresh_targets`, `targets`,
`request` and `poll` methods. Calls use bounded nonblocking mailboxes. They never
fall back to the editor recorder or another world.

`PlayProfileTarget` identifies the actual J10 world, player, source and binding;
inventory rows include the process ID and scene/role name. Exited incarnations
remain unavailable with a reason, with at most 128 inventory rows retained.
Requests against an unknown, unavailable or stale identity return `Refused`.
Outstanding controls are bounded at 32 active requests, with a bounded queue and
64 retained results per handle. Unknown or retired tickets return `None`.

Each single-use player owns one recorder for its world. Recording is off by
default, and its preallocated storage is created only on an explicit recording or
capture action. It uses `ara-profile::Config` defaults and retains at most 120
frames. Start while recording and Stop while idle return `Unchanged`. Snapshot
copies completed frames and preserves recording; Freeze copies completed frames
and stops it. Neither captures the current unfinished frame. Captures use the
ordinary `ara_profile::Profile`, summary, comparison and capture-writer APIs.

The trace contains measured player main-thread frame/control spans and input,
simulation and presentation phases. Entity counters come from that player's ECS
world. CPU durations include waits; unregistered worker lanes are not inferred
from status summaries. Recording metadata lives outside simulation state. The
Windows proof checks equal simulation hashes with recording off and on.

Native GPU samples carry the originating recorder token on the renderer's own
submitted completion. Frames in flight retain that identity through completion
retries; results never acquire the CPU frame in which readback happened. The
synchronous presentation path uses its own submission token. Only actual native
pass timestamps are imported. Unsampled paths, including headless players without
a graphics presenter, have no GPU pass entries.

Capture JSON encoding runs on a player-owned worker. Both sides use the existing
JSON-RPC framing; the owner's socket pump parses incoming frames away from the
Host tick, and a bounded worker decodes capture objects. No public Host RPC,
registry command, MCP method, C# SDK or native ABI is added. A private recorder
revision prevents an older snapshot response from undoing a later Stop state.
Backend diagnostics are published before terminal status and returned for display,
without requiring the UI to log them again. Busy preparation suspends the control
stall timer. Export/decoding failures report failure without restarting the world.

Focused verification: `ara-play` transport tests, `ara-play-host` profiling units,
`ara-player` recorder units and `--test profiling`, and `ara-runtime-render
--test profiling` for real offscreen two-frame GPU completion provenance. The
Windows C# Play suite protects the shared transport and J10 ingress contract.
