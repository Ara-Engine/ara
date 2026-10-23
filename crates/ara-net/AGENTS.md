# ara-net

Optional multiplayer adapter. Local-only applications depend on ara-sim and never
link Lightyear, Replicon, App, sockets, histories or network services.

## APIs and boundaries

- `Server<F>` drives an existing ara-sim Session through SessionAdapter in Listen
  or Dedicated mode. `SimulationFeature` belongs to ara-sim: one typed input,
  complete restorable ECS state and one step function. Gameplay imports no backend.
- `Client<F>` uses native Lightyear input histories, selective owner rollback and
  remote interpolation. Inject device/bot actions or attach an immutable input
  source. Sources run only when sampling new inputs, never during native replay.
- `PolicyRegistry` compiles field IDs/codecs from SchemaDb. Mixed public, owner and
  authority-only fields split before encoding; component visibility is enforced by
  Replicon. Quantization/interpolation are independent of rollback metadata.
  Plain nested values inherit their container's policy. If a nested type declares
  child policies, unannotated children stay private. Scoped collections are rejected.
- Native predicted movement requires complete relevant rollback fields in its state
  unit. Other components may mix private/public fields. Unsupported recursive or
  scoped collection layouts fail at registration rather than leaking private data.
- `IntentGate` admits only exposed simulation commands from authenticated owners:
  bounds, tick window, rate and sequence checks run before Session::submit. No
  document ops, edits or authoritative state messages exist on the client channel.
- `NetId` includes slot/epoch; Spawn contains prefab version and baseline identity.
  `Interest` combines spatial grid, ownership/team predicates. RelevantTo filters
  entire entities; PrivateTo filters only owner fields. Transport budgets use
  Lightyear's packet admission (control/ACK traffic is accounted separately).
- `Trace` captures arbitrary initial ara-sim snapshots and each applied boundary;
  replay reports the first diverging field/tick, then checks full state hashes.
  `Server::record(build, seeds)` records conditioner seeds; traces round-trip as CBOR.
- `Link` impairs actual encrypted datagrams below reliability. Directional latency,
  jitter, loss, duplication, reordering and bandwidth use recorded seeds. OS packet
  arrival order remains nondeterministic; this is not deterministic packet replay.
- `WorldControl` provides renderer-free multi-world tests. `register(host, control)`
  registers net.start/stop/status/conditions/profile/bots with Scope::Net and ara_net
  MCP actions. Composition must explicitly opt in; ara-host never depends on us.
- `PlayerPool` warms single-use ara-player processes away from the tick thread.
  LaunchPlan starts Host+N/Dedicated+N with trusted binary setup payloads, without
  cloning projects. `player_worker` receives one role/payload. Drop kills/reaps all
  owned processes. The editor owns viewport/shared-GPU-texture/input-focus wiring.
  Initialize engine/backend state before calling `player_worker`; its callback
  applies the prepared baseline/handles after Play assigns the role.

## Transport seam

Encrypted UDP/netcode is the default. JoinToken is private launch/auth data, never
replay/log data. A Steam Networking Sockets adapter belongs inside runtime/: replace
the Lightyear UDP I/O entities with a supported Link receiver/sender while retaining
the same schemas, native input/replication plugins and SessionAdapter. Do not expose
Steam or backend IDs to gameplay, add CMake, or invent a second reliable transport.
Steam authentication/relay availability is an adapter setup concern.

## Gates

Use `CARGO_BUILD_JOBS=2` on this shared machine. No dev server or build command.

```
cargo check -p ara-net -p ara-sim
cargo test -p ara-net -p ara-sim
cargo clippy -p ara-net -p ara-sim --all-targets -- -D warnings
cargo fmt -p ara-net -p ara-sim -- --check
```

gates.rs covers real conditioned UDP Local/Listen/Dedicated parity, late join,
native rollback, private inventory, replay, bandwidth and policy/intent/lifetime
contracts. process.rs re-execs its ignored worker test from a warm pool to run one
dedicated authority plus three headless bots; success/failure always reaps children.
Field profiles count generated field-codec payload; Link counts encrypted datagrams.
Do not compare rendered frames with authoritative same-tick checkpoints or repair
client state from expected test snapshots.

The composition root must call `ara_net::register(host, control)`; this optional
crate never makes ara-host depend on networking. Workspace members are globbed,
so no root manifest edit is needed. The integrating owner refreshes Cargo.lock.
