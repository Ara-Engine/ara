# Ara.Runtime

The gameplay API and the C# half of the host ABI. Loaded in the default ALC (dev) or
linked into the NativeAOT image (shipping). AOT-safe: `IsAotCompatible`, no reflection.

## Public API

- Data: `[Component]`, `[Resource]`, `[Cue]` partial structs (blittable fields only);
  `IComponent<T>.TypeIndex` etc. come from the generator. `Entity` (bevy bits; `default`
  is null), `Color`, `Rng` (PCG; `Rng.For(tick, key, entity)` is order-independent).
- Systems: `[Query(Order = n)] static void Run(ref A a, in B b, in Tick t, Entity e, ref Rng r, ref Commands c, in SomeResource res)`,
  `[With<T>]` / `[Without<T>]`. The generator emits the chunk loop.
- `Behaviour`: `OnSpawn`, `OnTick(in Tick)`, `OnFrame(in Frame)`, `OnReload`, `Get<T>()`,
  `Has<T>()`, `Commands`, `Rng`. `[Property] public partial T X { get; set; }` is an ECS
  column; other fields must be `[Transient]`.
- Commands: `[Command("game.X")] static partial class X { public static CommandResult Execute(ref CommandContext ctx, Entity target, ref C c, int value) }`;
  `ctx.Reject("reason")`, `ctx.Cue(...)`, `ctx.Commands`, `ctx.Rng`, `ctx.Caller`. `[Expose]`, `[Example]`.
- `World.Get/Read/Has/Resource` (behaviours and commands only), `Commands` (spawn,
  despawn, add, remove, cue; applied at the next sync point), `Log`.
- Metadata attributes: `Unit`, `Range`/`Min`/`Max`, `Default`, `Replicate`, `Tooltip`, `ColorPicker` (HDR/temperature),
  `Header`, `FormerlySerializedAs`, `Simulation`.
- Hosting (for loaders and generated code): `ScriptRuntime.Init/Enqueue`, `IModuleLoader`
  (`Load`, `SetHeat`, `RequestCompile`, ...), `StaticModuleLoader` (linked modules, dependencies
  first, as one swap), updates: `SwapUpdate` (one or more `ModuleSwap`s registered together
  through `Host.RegisterModules`, all or none), `DeltaUpdate` (L1/L2), `ErrorUpdate` (errors,
  L4), `ModuleBuilder`, `AraModuleAttribute`, `Host`, `Diag`.

- `ScriptRuntime` is an instance per Rust host. `Init` registers it by `HostInit.ctx`;
  every `ScriptApi` entry takes that handle. `Activate()` binds callbacks on one thread
  and restores its previous binding. No current host flows implicitly to async work.
  Loaders capture their owner with `Attach(runtime)` and enqueue directly to it.

## Gotchas

- `Generated/` is written by `cargo test -p ara-script --test generated` with
  `ARA_UPDATE_GENERATED=1`; never by hand. `AbiInfo.LayoutHash` must match the host.
- `ApplyPending` fills each `ReloadReport`'s generation, apply time (`ApplyUs`: registration,
  migration and the table swap) and cascade size before reporting it.
- Tables (`ModuleTables`) are swapped only at tick boundaries; `RunSystem` runs
  concurrently on task-pool threads and must stay lock-free.
- A fault disables the system/behaviour/command until the next reload (ARA0910–0912).
  A re-enabled behaviour keeps its objects (`BehaviourEntry.Resume`): no second `OnSpawn`.

- NativeAOT module types are shared: generated type indexes resolve through the active
  host's registration map. Engine indexes remain constant. Accessor scratch is scoped
  to each host entry, including nested bindings; shutdown never clears another host.

## Testing

`dotnet test tests/Ara.Runtime.Tests` (Rng, Entity bits, descriptors). End to end:
`cargo test -p ara-script`.
