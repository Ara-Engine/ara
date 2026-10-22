# R09 — Native multiplayer from day one

Research date: **2026-10-23**. Recommendations are engineering judgment; product/status claims below use current primary documentation, release notes, crate-registry metadata, and read-only reference-source inspection. No reference code was executed or built. API sketches describe a proposed snengine API, not an existing library. Performance targets are proposed acceptance criteria, not measured results.

## TL;DR

- **Default to server-authoritative snapshots, remote interpolation, and built-in owner prediction/reconciliation.** Dedicated and player-hosted sessions use the same gameplay model. Prediction is selective, not mandatory for every entity.
- **Single-player is a local authority session:** one simulation world, local typed input/commands, direct presentation access. No sockets, serialization, replica world, network history, or cloud dependency unless requested. Preserve multiplayer semantics without forcing multiplayer machinery.
- **Make ordinary gameplay network-ready structurally:** fixed-tick input-driven simulation; authoritative mutations; presentation outside simulation; schema-declared state. An annotation cannot repair arbitrary frame-driven, globally mutable gameplay.
- **Own the contract; reuse the implementation.** With R07's Bevy-crate direction, first evaluate Lightyear 0.31 + Replicon 0.45.2 behind snengine schemas. They now form a stack, not competing alternatives. Do not write a bespoke prediction framework first.
- **One schema generates wire codecs, language bindings, inspectors and agent descriptions.** Replication, prediction history, interpolation, recipient visibility and quantization are separate policies. Persistent editor patches are not the gameplay wire protocol.
- **Ship the workflow with the feature:** one-click server + N clients, adverse-network presets, bandwidth/reconciliation profiling, replay, and headless agent-driven convergence tests.
- **Keep transport and services replaceable.** Start with encrypted UDP/netcode through the selected stack, add Steam networking for PC friend-hosting, and integrate hosting through a small allocator contract. Unity Multiplay's 2026 deprecation and Rivet's actor-platform direction demonstrate why.
- **Defer deterministic whole-world rollback, lockstep, shared authority, host migration and MMO multiserver simulation.** Preserve extension points and tick/state contracts; do not make every game's architecture satisfy all five.

## How each reference engine / platform does it

### Unreal Engine

The core model is authoritative server worlds and replicated Actors/subobjects: selected properties flow to relevant clients; RPCs communicate intent/events; owning connections determine legal routing. Replication does not automatically reproduce arbitrary gameplay. Character Movement supplies specialized prediction, saved moves and server corrections. PIE supports standalone, listen-server and client/dedicated-server configurations, multiple worlds, separate-process testing and network emulation. Borrow both the ownership vocabulary and the testing experience. [Networking overview](https://dev.epicgames.com/documentation/unreal-engine/networking-overview-for-unreal-engine), [networked movement](https://dev.epicgames.com/documentation/unreal-engine/understanding-networked-movement-in-the-character-movement-component-for-unreal-engine), [PIE options](https://dev.epicgames.com/documentation/unreal-engine/play-in-editor-multiplayer-options-in-unreal-engine).

**Current-status correction:** UE **5.8 release notes declare Iris production-ready**, and add Iris support to the Network Prediction Plugin. Some general Iris pages still carry Experimental banners; the newer release notes take precedence. Iris separates quantized replicated state from gameplay objects, shares work across connections, and supports filtering/prioritization. It is a replication system, not a complete arbitrary-game prediction solution. Mover supports rollback-oriented movement through Network Prediction or Chaos networked physics; the 5.8 notes still describe work toward leaving Experimental. Do not repeat the older claim that Iris and Network Prediction cannot work together. [5.8 release notes](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-8-release-notes), [Iris design](https://dev.epicgames.com/documentation/unreal-engine/introduction-to-iris-in-unreal-engine), [Mover](https://dev.epicgames.com/documentation/unreal-engine/mover-in-unreal-engine).

GAS adds a narrower, valuable precedent: predict an ability locally using prediction keys, let the server accept/reject it, and undo supported predicted effects on rejection. Its prediction does not cover every arbitrary operation. snengine should borrow explicit prediction identities and reversible effects without requiring a GAS-sized framework. [GAS overview](https://dev.epicgames.com/documentation/unreal-engine/understanding-the-unreal-engine-gameplay-ability-system), [prediction-key contract](https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/GameplayAbilities/FPredictionKey).

### Unity

**Netcode for GameObjects (NGO)** uses NetworkObjects, NetworkBehaviours, NetworkVariables, network prefabs and generated RPC plumbing. **NGO 3.1.0's current manual still distinguishes client anticipation from full rollback/replay prediction**; implementing the latter remains user work. This was checked against the October 2026 package documentation, not inferred from old 2.x tutorials. **Netcode for Entities** provides ghosts, generated field serialization, interpolated/predicted ghosts, input histories and resimulation. Unity 6.3's package page currently lists **1.14.3**; package versions differ by editor branch. Borrow Entities' scheduling model without making developers choose a different object model to obtain responsive multiplayer. [NGO 3.1 anticipation](https://docs.unity3d.com/Packages/com.unity.netcode.gameobjects@3.1/manual/advanced-topics/client-anticipation.html), [Entities package status](https://docs.unity.com/en-us/engine/6000.3/manual/packages-list/packages-all/pack-safe/com-unity-netcode), [prediction mechanics](https://docs.unity3d.com/Packages/com.unity.netcode@1.14/manual/intro-to-prediction.html).

**Distributed authority** assigns object authority among clients, backed by a lightweight state/routing service; it does not secretly run a trusted gameplay simulation. It changes cheating and failure assumptions. **Multiplayer Play Mode** supplies multiple local editor players; current Unity 6000.5 documentation lists 2.0.2. Sessions/Multiplayer Services coordinate connectivity and services separately from gameplay replication. That separation is useful; the package/topology decision matrix is not the desired authoring experience. [Distributed authority](https://mp-docs.dl.it.unity3d.com/netcode/2.1.1/terms-concepts/distributed-authority/), [Play Mode status](https://docs.unity.com/en-us/engine/6000.5/manual/packages-list/packages-all/pack-safe/com-unity-multiplayer-playmode), [session networking](https://docs.unity.com/en-us/mps-sdk/manage-session-network-connection).

### Godot

SceneMultiplayer implements the high-level SceneTree API over MultiplayerPeer implementations including ENet, WebRTC and WebSocket. MultiplayerSpawner handles configured scene spawning; MultiplayerSynchronizer replicates configured properties from their authority and supports per-peer visibility. RPC annotations express sender/transfer rules. This makes simple co-op approachable but does not establish a general prediction/reconciliation scheduler. Object/resource values and process-local IDs have replication limitations. Copy declarative spawning and visibility, replace path-sensitive identity with stable mapped handles, and make prediction a first-party facility. [High-level multiplayer](https://docs.godotengine.org/en/stable/tutorials/networking/high_level_multiplayer.html), [Synchronizer](https://docs.godotengine.org/en/stable/classes/class_multiplayersynchronizer.html).

Local source confirms configuration attachment via `object_configuration_add/remove`, independent replication/delta intervals, and visibility filters in `multiplayer_synchronizer.cpp`; `scene_multiplayer.cpp` dispatches RPC, path-cache and replication traffic separately. This is useful layering, not a reason to equate scene replication with simulation rollback. [Pinned Godot source](https://github.com/godotengine/godot/tree/9c2e39242e20846f84d990fe457a5e146b77fa36/modules/multiplayer).

### O3DE

The Multiplayer Gem is particularly relevant: server authority, autonomous predicted proxies, read-only client proxies, network inputs, delta properties, rewind and generated RPCs. XML auto-components generate C++ and editor/script bindings. In the inspected source, `LocalPredictionPlayerInputComponent.AutoComponent.xml` declares input/correction RPC directions; its controller compares state hashes and replays history. `NetworkTransformComponent.AutoComponent.xml` independently marks predictable and rewindable fields. Borrow those distinctions and input histories; put metadata in the common Rust schema rather than a second XML component language. Defer O3DE's multiserver authority machinery. [Gem overview](https://docs.o3de.org/docs/user-guide/networking/multiplayer/overview/), [auto-components](https://docs.o3de.org/docs/user-guide/networking/multiplayer/autocomponents/), [pinned implementation](https://github.com/o3de/o3de/tree/2462b90ee389939336931a14e234fc4e3b4cd534/Gems/Multiplayer/Code).

### Flax

Flax provides NetworkReplicator, generated C#/C++ serializers/RPCs, object registration/spawning, ownership, replication hierarchy and configurable rates. `DirtyObject` requests prompt replication. Source includes NetworkTransform's Default, Interpolation and Prediction modes, plus RPC sender/recipient context. That is more than a socket wrapper, but does not establish that arbitrary gameplay/physics can be restored and resimulated automatically. Borrow concise field/RPC declarations and relevancy hierarchy; evaluate prediction guarantees independently. [High-level networking](https://docs.flaxengine.com/manual/networking/high-level.html), [pinned networking source](https://github.com/FlaxEngine/FlaxEngine/tree/394658f3cba5002c6cc16f054b1443a1b6022f05/Source/Engine/Networking).

### Roblox and UEFN/Verse

**Roblox is the product benchmark:** replicated world objects, server/client script placement, RemoteEvents/RemoteFunctions, integrated hosting and multiplayer testing are ordinary platform concepts. Traditional automatic physics ownership can delegate simulation to clients; this is not equivalent to trustworthy server authority. Current Roblox documentation also describes a newer **server-authority model with fixed simulation, input prediction, rollback/resimulation and streaming**, enabled through Workspace settings. Documentation presence does not establish universal rollout/default status; that was not independently verified. Borrow the cohesive workflow and current input/state separation, not blanket client ownership. [Client/server security](https://create.roblox.com/docs/scripting/security/client-server-boundary), [ownership modes](https://create.roblox.com/docs/reference/engine/enums/NetworkOwnership), [server authority](https://create.roblox.com/docs/projects/server-authority).

**UEFN** inherits Fortnite's sessions, replication and production backend; creators program devices/entities through Verse rather than provisioning a separate netcode stack. Epic's current camera documentation explicitly says Verse logic runs on the server. This simplifies authority reasoning but does not make arbitrary Verse local input latency disappear, nor imply every ordinary Verse variable replicates. snengine should make common operations similarly native while exposing an explicit predicted simulation tier for responsive gameplay. [Current Verse execution statement](https://dev.epicgames.com/documentation/fortnite/camera-types-in-unreal-editor-for-fortnite).

### Photon, Mirror and FishNet

**Fusion 2** combines networked properties, fixed network ticks, snapshot buffers and client prediction/reconciliation; Server/Host and Shared are distinct authority choices. **Quantum 3** instead supplies a deterministic simulation with input synchronization and predict/rollback; it should not be described as merely waiting in strict lockstep. This is powerful for compatible games, but requires authoring inside its deterministic model. Both remain documented products; no commercial terms or production performance were benchmarked. [Fusion topologies](https://doc.photonengine.com/fusion/v2/manual/network-topologies), [Fusion prediction](https://doc.photonengine.com/fusion/v2/tutorials/host-mode-basics/3-prediction), [Quantum frames](https://doc.photonengine.com/quantum/v3/manual/frames).

**Mirror** offers a familiar NetworkBehaviour/SyncVar/Command/RPC approach; its prediction documentation explicitly labels the discussed prediction work experimental. **FishNet** documents input replication and reconciliation methods, prediction rigidbodies, tick scheduling and state forwarding. Neither should be dismissed as obsolete, and neither changes Unity's fundamental simulation/physics obligations. Their useful lesson is that transform sync is easy to demonstrate; robust prediction requires an authored state/input contract. [Mirror prediction](https://mirror-networking.gitbook.io/docs/manual/general/client-side-prediction), [FishNet prediction](https://fish-networking.gitbook.io/docs/guides/features/prediction/creating-code/controlling-an-object).

### SpacetimeDB; Hytale/Minecraft pattern

**SpacetimeDB**, now presented as Spacetime, runs typed server logic against database tables; transactional reducers and subscriptions combine authority, persistence and client synchronization. BitCraft Online is a concrete first-party example. Its current site advertises Rust, C# and TypeScript server logic. Excellent inspiration for typed commands, queries and persistent-world backends; not evidence that database subscriptions replace local prediction, physics or lag compensation. Keep it an optional backend rather than snengine's simulation substrate. [Platform and BitCraft example](https://spacetimedb.com/), [documentation](https://spacetimedb.com/docs/).

**Hytale** currently documents client+server requirements, a server manual, and opening a single-player world to friends, including relay fallback. This supports the local-authority product pattern associated with Minecraft. Do not inherit either game's process layout or protocol overhead as an engine requirement. Hytale's 2019 architecture article is historical, not sufficient evidence of its 2026 implementation. [Hardware model](https://hytale.com/news/2025/12/hytale-hardware-requirements), [joining friends](https://support.hytale.com/hc/en-us/articles/45324238811291-Joining-Friends), [current server manual](https://support.hytale.com/hc/en-us/articles/45326769420827-Hytale-Server-Manual). Minecraft's exact current integrated-server internals were not independently verified in this survey.

### Rust ecosystem: current shortlist

Versions below were read directly from the crates.io API on the research date; they do **not** constitute one tested compatible dependency set.

| Project | Verified capability/status | snengine role |
|---|---|---|
| **Lightyear 0.31.0** | Bevy 0.20; inputs, prediction, interpolation, transport adapters; current world replication uses Replicon. Recent migration notes identify feature gaps, including authority switching and some delta/priority features. | First candidate for the integrated implementation. Pin and test actual released behavior. [Project](https://github.com/cBournhonesque/lightyear), [release notes](https://github.com/cBournhonesque/lightyear/releases). |
| **bevy_replicon 0.45.2** | Server-authoritative replication, entities/events and visibility; prediction belongs to an integration layer. | Reuse beneath Lightyear, not a second parallel replicator. [Project/changelog](https://github.com/simgine/bevy_replicon). |
| **renet 2.0.0 / renet2 1.0.0** | Message channels, delivery guarantees and netcode authentication/encryption integrations. Renet2 is a separate fork, with additional socket backends. | Alternative lower layer if the higher-level stack fails; neither alone supplies gameplay replication/prediction. [Renet](https://github.com/lucaspoffo/renet), [Renet2](https://github.com/UkoeHB/renet2). |
| **naia-server 0.25.0** | ECS-agnostic replication, typed messages/protocols, scope/rooms, native UDP/browser WebRTC. Current README says native mobile support is incomplete. | Strong schema/interest-management precedent; mobile support is a gating concern. [Naia](https://github.com/naia-lib/naia). |
| **GGRS 0.13.0** | Engine-agnostic P2P rollback, save/load/advance requests and synchronization tests. | Later fighting-game/deterministic rollback plugin. Hosted example matchmaking is reported offline; library availability is separate. [GGRS](https://github.com/gschup/ggrs). |
| **matchbox_socket 0.14.0** | Native/Wasm WebRTC channels and signaling, GGRS integration. | Optional P2P/browser transport; signaling is not the TURN relay needed when direct paths fail. [Matchbox](https://github.com/johanhelsing/matchbox). |
| **quinn 0.11.12** | Rust QUIC, reliable streams plus unreliable datagrams; I/O-free `quinn-proto`. | Credible alternative/default dedicated transport after a measured integration comparison. Not netcode by itself. [Quinn](https://github.com/quinn-rs/quinn). |
| **steamworks 0.13.1 / game-networking-sockets 0.3.0** | Rust bindings to Steamworks / standalone Valve GameNetworkingSockets respectively. | PC connectivity adapter candidates; FFI/SDK/platform validation remains. Standalone GNS does not grant Steam relay access. [Steamworks Rust](https://github.com/Noxime/steamworks-rs), [GNS binding](https://github.com/hussein-aitlahcen/gns-rs), [Valve](https://github.com/ValveSoftware/GameNetworkingSockets). |

## Netcode models and state of the art

**One default: authoritative state replication with selective prediction.** Snapshot interpolation renders remote entities slightly behind the server from buffered authoritative states. A local player runs eligible simulation ahead from tick-stamped inputs; corrections restore authoritative state at a known tick and replay unacknowledged inputs. These are complementary parts of one model. Start without prediction for a door/inventory; use built-in prediction for movement. Server authority is independent of whether the server is dedicated or owned by a player.

| Model | Best fit | Main cost / decision |
|---|---|---|
| Server snapshots + interpolation | Co-op, survival, RPGs, authoritative worlds | Server CPU/bandwidth; remote rendering delay. **Default foundation.** |
| Same model + prediction/reconciliation | Shooters, action, platformers, racing | Replay-safe gameplay, dependency histories and corrections. **First-party selective capability.** |
| Deterministic lockstep | Large RTS, deterministic strategy, some turn-based games | All participants must reproduce identical state; input delay/stalls and hidden-state concerns. **Defer plugin.** |
| Deterministic input rollback | Fighting games, small competitive action games | Whole relevant simulation must save/restore and converge; bounded rollback CPU. **GGRS-like plugin later.** |
| Shared/distributed authority | Trusted social/sandbox sessions, collaborative interactions | Authority transfer, disconnect recovery and weaker anti-cheat. **Defer.** |

“Rollback” in server reconciliation is not automatically GGPO-style whole-game P2P rollback. Likewise, strong typed RPCs are not enough for responsiveness. The current state of the art is an integrated timeline: input scheduling, selective histories, predicted spawning, correction smoothing, relevancy, prioritization and explainable diagnostics. Photon, O3DE, Unreal, Roblox and Lightyear expose different portions of that system.

**Determinism has levels.** Require reproducible replay under the same binary/platform/configuration for supported simulation features. Do not promise cross-platform bitwise equality by default. Rapier's current `enhanced-determinism` offers stronger cross-platform behavior with feature constraints; current documentation specifically excludes combination with `simd8`. Jolt offers `CROSS_PLATFORM_DETERMINISTIC`, but calls out identical settings, query/callback ordering and testing caveats. Neither option makes scripts, ECS scheduling, RNG, asset loading or external services deterministic. [Rapier features](https://www.rapier.rs/docs/user_guides/templates/getting_started/), [Jolt determinism](https://github.com/jrouwe/JoltPhysics/blob/master/Docs/Architecture.md).

For launch, predict a kinematic character controller against static geometry; replicate complex dynamic rigidbodies authoritatively. Expand to bounded predicted physics islands after measuring rollback cost. Restoring transforms alone is insufficient: velocities, sleeping/contact/constraint state, gameplay timers and relevant neighbors can affect the next step. Keep a real physics snapshot/restore interface even if initial use is narrow.

## Recommendations for snengine

### 1. Local authority is the cheap specialization

Use a `Session` with an authority simulation, local player identities, input sources and presentation views. In **Local** mode there is one simulation world and direct typed command dispatch at the same tick boundaries used online. Rendering reads an extracted view; there is no client replica or transport loop. Enable history only for replay/testing or prediction. Compile network codecs/transport modules out of a strictly single-player export; allocate no per-connection state when networking is absent.

In **Listen** mode the same authoritative world serves remote clients; its local player uses the direct path. In **Dedicated** mode there is no local player, camera, renderer, audio device or editor dependency. Remote clients maintain an authoritative receive baseline plus predicted/interpolated views. Never run the local player's simulation twice on a listen host.

The achievable promise is **zero network serialization and optional networking machinery**, not literally zero architectural overhead. Fixed ticks and explicit inputs also benefit single-player. Full dual-world/in-memory-transport sessions remain a correctness test mode. Starting online from an offline world requires an explicit tick-boundary session transition, baseline capture and join policy; it need not be a launch feature.

### 2. Separate replicated state, rollback state and presentation

The common schema should express:

- Stable component/field/command IDs; numeric bounds; maximum collection sizes; prefab/asset references; codec version and compatibility hash.
- Replication direction and recipients: authority to relevant clients, owner-only/private state, initial-only/static state, update priority and dormancy.
- Wire quantization, interpolation mode and correction tolerances. Quantize once per entity/tick when possible; delta against an acknowledged baseline, not the last packet sent. Recover with bounded full baselines.
- Independent **rollback participation**: include all future-affecting state in a prediction group, even private timers/RNG state not sent to other players. Exclude presentation caches and service handles.

Persistent asset IDs, local generational entity handles and session-scoped network IDs are different types. Map them explicitly. A network identity includes a generation/epoch so late packets cannot resurrect a reused entity. References survive out-of-order spawning via bounded deferred resolution. Spawn messages identify cooked prefab versions plus initial state; joining clients receive a consistent baseline and subsequent changes.

R01/R03 propose shared editor command/patch machinery. **Share schemas, target identities, validation infrastructure and recording primitives, not unrestricted mutation access or identical protocols.** Editor JSON patches/undo transactions are not bandwidth-efficient snapshots and must not become public game RPCs. A client requests `UseMedkit`; it cannot send `SetHealth(1000)`. Predicted gameplay execution is tick-indexed, unlike ordinary document undo.

### 3. Author ordinary features through typed inputs and commands

The following proposed TypeScript API uses a typed schema builder. It registers schema descriptors with Rust; compiler tooling emits validators/codecs/bindings from that description. Built-in engine schemas originate in Rust derives. Each type has one owning declaration, not hand-maintained Rust and TS copies.

```ts
// Existing single-player feature: no network superclass or connection checks.
const Vitality = component("game.Vitality", {
  hp: field.u16({ default: 100, max: 100 }),
  medkits: field.u8({ default: 1, max: 9 }),
});
const MedkitUsed = cue("game.MedkitUsed", { actor: field.entity() });

const UseMedkit = command("game.UseMedkit", {
  input: input.unit(),
  target: Vitality,
  execute(ctx, actor) {
    if (!ctx.alive(actor) || actor.hp === 100 || actor.medkits === 0)
      return ctx.reject("cannot_heal");
    actor.medkits -= 1;
    actor.hp = Math.min(100, actor.hp + 25);
    ctx.cue(MedkitUsed, { actor: actor.id }); // committed presentation cue
  },
});

// A human, UI, controller or test agent uses the same typed command.
player.submit(UseMedkit, player.actor, {});
```

To make it multiplayer, keep the handler and attach policy:

```ts
replication(Vitality, {
  hp: { audience: "relevant", mode: "state" },
  medkits: { audience: "owner", mode: "state" },
});
expose(UseMedkit, {
  executeOn: "authority",
  caller: "owner",
  delivery: "reliable",
  rateLimit: { count: 2, windowTicks: 60 },
});
// Local, listen and dedicated sessions all invoke execute at a tick boundary.
```

The server derives the caller from the authenticated connection, verifies target ownership and gameplay preconditions, and deduplicates command sequence IDs. Reliable delivery does not guarantee an effect occurs once across reconnects. Inventory persistence requires a separate idempotent transaction boundary. Durable HP/inventory state replicates; a transient cue cannot be the source of truth for late joiners.

For movement, expose a **simulation feature** rather than RPC-per-frame calls:

```ts
const Movement = simulationFeature({
  input: MoveInput,              // typed action values, tick-indexed
  state: [MotorState],           // complete restorable local state
  step: stepMotor,               // same function offline/server/predicting owner
});
network(Movement, {
  authority: "server",
  prediction: "owner",
  remoteView: "interpolate",
});
```

Built-in motor/input types must already satisfy this contract. Predicted commands/cues need stable `(player, tick, sequence)` identities; replay must not duplicate sound, VFX, analytics or purchases. Emit/retract visual speculative cues separately from confirmed irreversible effects. Prediction group dependencies must be explicit: a health/cooldown value influencing movement cannot silently remain outside its history. Unsupported access should produce a useful diagnostic, not plausible divergence.

### 4. Make time, ownership and relevance explicit

Start with a configurable **60 Hz simulation**, **20 Hz snapshot** prototype; tune per project and measured CPU/bandwidth. These are starting values, not universal engine constants. Fixed simulation, input sampling, replication and rendering have separate rates. Apply network arrivals at tick boundaries; correct drift by controlled scheduling, never arbitrary variable simulation deltas. Define missing/late input rules, bounded future lead and catch-up work.

Separate **authority** (may commit state), **input ownership** (may request actions) and **visibility** (may receive state). A player owning an avatar does not own its health truth. Launch with one authority, authenticated player IDs and explicit disconnect/despawn policy; retain an authority epoch without implementing migration.

Ship coarse spatial-grid interest management plus explicit owner/team/room predicates and per-connection byte budgets. Relevancy transitions require spawn baselines and orderly despawns; visibility also enforces information security. Dormancy and dirty ticks reduce work, but unchanged state may still need acknowledged confirmation for prediction. Defer sophisticated graph/custom priority systems until a workload needs them.

Lag compensation is separate from reconciliation: for a later shooter package, retain bounded authoritative hitbox history and validate/clamp client action timestamps before rewinding a query scene. Do not rewind the live world or let clients choose arbitrary historical hits. Ship the history hook early; ship genre-specific hit validation when required.

### 5. Reuse Rust netcode; own a narrow integration boundary

First prove **Lightyear 0.31 + Replicon on Bevy 0.20**, matching R07. Generate component registration and codecs from snengine schemas; keep upstream types out of project documents and public scripts. Use upstream input/timeline/prediction facilities where adequate. A networking adapter should expose connection state, typed messages, authoritative state checkpoints and diagnostics—not an elaborate universal abstraction over every topology.

The gate is one character, a door and an inventory: offline, listen and dedicated must behave consistently; correction, private state, late join, dropped packets and replay must work. Current Lightyear migration notes mean advertised delta/authority features require released-version checks. If this fails, evaluate Naia or a Renet/Quinn-based narrower implementation against the same tests. Avoid simultaneously adopting multiple competing state replicators.

### 6. Transport, connectivity and online services

**Default implementation path:** use the chosen stack's established encrypted UDP/netcode transport; do not invent reliability, cryptography or congestion control. Keep snapshots/input redundant and unreliable-sequenced; use bounded reliable channels for spawn/lifecycle and discrete commands. Separate traffic classes so stale movement or bulk assets cannot block urgent gameplay. Respect negotiated MTU and fragmentation limits.

**QUIC is credible, not synonymous with TCP:** unreliable QUIC DATAGRAMs can carry snapshots while streams carry reliable messages; encryption/congestion control are built in. Streams still have ordering within each stream; everything shares connection bandwidth. Quinn is a strong Rust option but raw QUIC does not provide matchmaking/NAT traversal. Benchmark an adapter before changing the default transport path. WebTransport offers browser client/server streams/datagrams; it is not raw Quinn wire interoperability or P2P NAT traversal. WebRTC adds browser-friendly peer connectivity but needs signaling, ICE/STUN and often TURN. Browser export is not a launch requirement; keep adapters possible rather than shipping all of them. [QUIC DATAGRAM RFC](https://www.rfc-editor.org/rfc/rfc9221.html), [WebTransport specification](https://www.w3.org/TR/webtransport/), [Quinn](https://github.com/quinn-rs/quinn), [Matchbox](https://github.com/johanhelsing/matchbox).

For PC friend-hosting, integrate **Steam Networking Sockets/SDR** first when a game ships on Steam. Valve's open-source library provides transport, encryption and traversal primitives, but Steam authentication/signaling/relay services have separate availability conditions. **EOS P2P** is an alternative service adapter with relay fallback; the official relay enum verifies fallback behavior, but current full P2P page contents were inaccessible to this fetch, so exact encryption/platform/version guarantees remain unverified here. Keep SDK claims scoped accordingly. [Valve capabilities and service distinction](https://github.com/ValveSoftware/GameNetworkingSockets), [EOS relay policy](https://dev.epicgames.com/docs/en-US/api-ref/enums/eos-e-relay-control).

The engine should ship local/LAN/direct sessions, join descriptors, server lifecycle, a small lobby/session UI and one documented service integration. It should integrate identity/lobbies, matchmaking, relays and fleet hosting through separate interfaces. Never require an snengine account or proprietary relay to run a game. Direct Internet hosting needs public reachability; a join code alone does not solve NAT, and CGNAT makes relay fallback essential for dependable player hosting.

Current provider evidence:

- **Unity Multiplay hosting was deprecated April 1, 2026**, with a limited migration exception. Unity Matchmaker now supports third-party hosting via Cloud Code; Relay/Lobby/Distributed Authority are separate services. [Deprecation notice](https://status.unity.com/info_notices/362941), [current host integration](https://docs.unity.com/en-us/mps-sdk/matchmaking/mm-hosting-providers).
- **Edgegap** still documents container-based game-server orchestration; **Amazon GameLift Servers** offers managed EC2, Linux containers and Anywhere; **PlayFab Multiplayer Servers** remains documented. Treat geography, price, minimum capacity and operational limits as per-game procurement decisions, not permanent engine assumptions. [Edgegap](https://docs.edgegap.com/), [GameLift](https://docs.aws.amazon.com/gameliftservers/latest/developerguide/gamelift-intro-flavors.html), [PlayFab](https://learn.microsoft.com/en-us/gaming/playfab/features/multiplayer/servers/).
- **Rivet's current product/changelog centers actors, workflows, compute and agent infrastructure.** It is not safe to recommend its historical game-server offering unchanged. Exact legacy game-hosting availability was not verified; reassess capabilities for an actual deployment. [Current product history](https://rivet.dev/blog/).

Ship a renderer-free Linux server target with config/env/CLI ports, structured logs, readiness, graceful drain and allocator callbacks. Container images are a packaging option, not a runtime dependency. Keep provider SDK credentials on the server/control plane. On mobile, treat backgrounding as a possible disconnect: stop inputs, preserve reconnect identity, expire ownership safely, and fetch a fresh baseline on resume. iOS normally suspends background apps; cellular changes, IPv6/CGNAT, jitter and energy costs require real-device testing. Do not promise sustained mobile listen-hosting. [Apple background execution](https://developer.apple.com/documentation/xcode/configuring-background-execution-modes), [Android connectivity energy](https://developer.android.com/develop/connectivity/preserving-battery).

### 7. Security and scripting are contracts, not decorations

Server validation includes sender/target authorization, tick windows, movement limits, cooldowns, resource budgets, finite numeric values, collection bounds and command rates. Authenticate before allocating expensive per-client worlds; cap decode/history queues and decompression sizes. Clients send inputs and intent, not damage totals or authoritative transforms. Prediction improves feel; the client remains untrusted. A listen host can cheat, and encrypted transport does not fix that. Owner-only/interest filtering must avoid sending hidden information that wallhacks can read. [Roblox's concrete validation guidance](https://create.roblox.com/docs/scripting/security/client-server-boundary).

**TS:** inferred schema builders plus generated bindings/runtime validators; plain TS types erase and cannot define a wire protocol alone. **Luau:** generated typed APIs plus runtime schemas/validators, not replication of arbitrary tables. **C#:** attributes and source generation can express the same schema. **Rust:** derives register strongly typed state/input/command codecs. Do not let language-specific object graphs cross the boundary. All languages require bounded wire types and typed entity/asset handles.

Persist gameplay state in registered components, not VM closures, arbitrary globals, pending promises or native handles. Prediction executes a synchronous, restricted simulation surface with tick time and deterministic RNG; external I/O returns as recorded commands on a later tick. JavaScript/V8 heaps are not assumed snapshotable. Hot reload changes invalidate affected prediction/replay histories; restart/resynchronize sessions when schema/code compatibility cannot be preserved.

### 8. Editor and agent testing are part of launch scope

The Play menu should choose **Local / Host + N clients / Dedicated + N clients**, player input routing and a named network profile. Start with separate game processes for isolation and realistic server-only errors; add fast multi-world mode later if valuable. Show authority/input owner, server tick, predicted tick, interpolation delay, and whether the selected field is authoritative, predicted or rendered.

Network simulation must support directional latency, jitter, packet/burst loss, duplication, reordering, bandwidth limits, disconnection/reconnect and clock skew with recorded seeds. Apply datagram impairment below reliability/congestion handling for realistic transport behavior; a logical-message simulator alone misses retransmission effects. Provide a fast virtual-link mode and a socket integration mode, explicitly labeled.

The profiler attributes bytes to entity/component/field and traffic class; show baseline/delta size, visibility causes, packet loss, input age, rollback counts/depth, resimulation CPU and correction magnitude. A correction is not automatically a bug: diagnostics must distinguish expected unknown-input correction from repeatable divergent simulation.

One replay container should record build/schema/asset hashes, initial snapshots, accepted inputs/commands, RNG state/seeds, external outcomes and optional transport trace. Gameplay replay and packet replay are distinct tracks. Canonical state hashes compare the same authoritative tick and projection; do not compare rendered/predicted frames or private server state to partial client views.

Agent tools should create sessions, attach headless clients/bots, drive typed actions, step virtual time and assert outcomes. After stopping inputs and flushing corrections, assert replicated-state convergence; do not demand equality while clients predict ahead. Include late joins, rejected commands, ownership changes/disconnects, stale references, private-state visibility and replay reproduction. Replay-test prediction by forcing save/restore/resimulation and checking the same-tick state, not through endless screenshot smoke tests. Artifacts should identify the first differing field/tick and preserve the minimal failing action sequence.

## Requirements this imposes on the core architecture

These contracts must exist before broad gameplay APIs stabilize:

1. **World/session isolation:** multiple worlds and headless operation; no global “current player,” renderer singleton or implicit local camera in gameplay.
2. **Explicit scheduling:** fixed simulation; separate presentation/network phases; tick-boundary input, structural mutation and asynchronous-result application; bounded catch-up.
3. **Schema ownership:** stable IDs, versions, bounded wire types, visibility, quantization and generated native/script validators; private state is expressible without separate component classes.
4. **State lifecycle:** prefab manifests, stable network-handle mapping, generations, deterministic spawn/despawn ordering, late-join baselines and deferred reference resolution.
5. **History/restore:** opt-in complete simulation state, including resources/RNG/physics and lifecycle; replication and rollback selection remain independent.
6. **Replay-safe effects:** typed input/command records, deterministic RNG streams per system/entity, explicit timers and effect identities; presentation cannot mutate simulation accidentally.
7. **Authority boundaries:** capability-checked writes, authenticated caller context, distinct authority/input owner/recipient concepts; editor tools stay outside public gameplay permissions.
8. **Efficient observation:** change ticks, read-only snapshot extraction, acknowledged baselines, interest queries, backpressure and bounded memory. Avoid mandatory per-write editor transactions in hot simulation loops.
9. **Infrastructure seams:** transport/session allocation adapters, platform suspend/resume and reconnect lifecycle; network I/O feeds bounded queues instead of borrowing the world across async waits.
10. **Inspectable testing:** deterministic stepping, field-level hashes/diffs, structured traces, reproducible seeds, headless clients and shared replay artifacts. All optional buffers/services disappear from ordinary local sessions.

This retains R01's stable identity/reflection direction and R03's typed agent commands, while applying the owner's newer Rust and native-multiplayer requirements. R07's suggestion to defer game-specific topology remains appropriate for advanced modes; basic authority, input and state contracts cannot now be deferred.

## Open questions / risks

- **First proving game:** recommend a small third-person co-op/action scene with moving players, a shared door, private inventory and a late joiner. It exposes more architectural failures than transform-only demos without requiring MMO infrastructure.
- **Physics and scripting:** evaluate correction/resimulation on x86 Windows/Linux and ARM macOS before promising predicted rigidbody interaction; test mobile once supported. Script-owned hidden state is a greater risk than the transport choice.
- **Dependency volatility:** Lightyear's recent Replicon migration is promising but young; no compatibility/performance/security audit was performed here. Published version numbers are not proof of production suitability.
- **Cost scope:** default player/entity counts and bandwidth budgets remain product decisions. Do not sell “Unreal-level fidelity” as “every rendered object replicates”; simulation and visual density must decouple.
- **Service uncertainty:** no hosting deployment, relay purchase or mobile SDK certification was performed. Provider billing, quotas, availability and legacy Rivet support require re-verification when selected.
- **Hard limit on effortless conversion:** arbitrary shared-memory single-player code still needs cleanup. Enforce the right input/state/presentation boundaries in starter templates so most future conversions add policies rather than rewrite gameplay.

## Sources

Primary sources are linked at the claims they support. Particularly consequential current-status evidence:

- [UE 5.8 release notes](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-8-release-notes): Iris production readiness, Network Prediction integration, Mover status; supersedes older experimental/compatibility claims.
- [NGO 3.1 manual](https://docs.unity3d.com/Packages/com.unity.netcode.gameobjects@3.1/manual/advanced-topics/client-anticipation.html): anticipation versus full prediction, fetched directly and inspected as HTML text.
- [Roblox server authority](https://create.roblox.com/docs/projects/server-authority): fixed simulation, prediction and resimulation requirements; rollout universality remains unverified.
- [Lightyear 0.31 source](https://github.com/cBournhonesque/lightyear/tree/0.31.0), [Replicon changelog](https://github.com/simgine/bevy_replicon/blob/master/CHANGELOG.md): current architecture and integration caveats.
- [crates.io API](https://crates.io/api/v1/crates/lightyear): versions queried through `/api/v1/crates/{name}` for each listed crate on 2026-10-23; metadata, not a resolved stack test.
- [Unity hosting deprecation](https://status.unity.com/info_notices/362941), [Rivet changelog](https://rivet.dev/blog/): current service changes; no inference that all other multiplayer services shut down.
- Local read-only snapshots: Godot `9c2e39242e20846f84d990fe457a5e146b77fa36`; Flax `394658f3cba5002c6cc16f054b1443a1b6022f05`; O3DE `2462b90ee389939336931a14e234fc4e3b4cd534`. These are inspected commits, not claimed latest releases. UnityCsReference is not the source of Unity's package-level netcode findings.
- Internal context read, not modified: `01-core-architecture.md`, `03-ai-native-design.md`, `04-editor-ux-tools.md`, `07-rust-path.md`. `08-platforms-export.md` was absent at the context check.

**Final implementation handoff**

**Default:** one server-authoritative snapshot model, selective owner prediction/reconciliation, remote interpolation. **API conversion:** keep an input-driven handler, add replication/authority policy, change session configuration:

```ts
replication(Vitality, { hp: relevantState(), medkits: ownerState() });
expose(UseMedkit, { executeOn: "authority", caller: "owner" });
await session.start({ mode: "listen", localPlayers: 1 });
// Existing player.submit(UseMedkit, player.actor, {}) stays unchanged.
```

**Editor/testing:** one-click dedicated server + two clients, a reproducible adverse-network preset, bots through the action API, same-tick convergence assertions, and replay with the first differing field. **Core requirements:** isolated headless worlds; fixed tick/presentation separation; stable typed schemas/identities; authority capabilities; complete opt-in history/restore; replay-safe effects/RNG; change tracking/baselines/relevancy; transport/service seams; inspectable agent tests.

**The five multiplayer decisions hardest to change later:**

1. Who may author authoritative state, and how client intent enters simulation.
2. Whether gameplay is tick/input-driven with separate presentation and replay-safe side effects.
3. Which state—including scripts, resources, physics and lifecycle—can be captured and restored.
4. Stable schema/entity/prefab identities, wire compatibility and private-state visibility rules.
5. Whether local, listen, dedicated and test worlds share semantics without global process assumptions or mandatory online services.
