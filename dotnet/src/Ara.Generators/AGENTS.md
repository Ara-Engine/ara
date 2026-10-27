# Ara.Generators

The Roslyn incremental generator every gameplay module uses (netstandard2.0, Roslyn 4.14).

## What it emits

- `[Component]`/`[Resource]`/`[Cue]` structs: `IComponent<T>` (etc.) with a script index set
  at registration in the active host's map (also safe for shared NativeAOT types),
  and `__AraLayout()` (measured field offsets).
- `[Query]` methods: `__AraRun_<Method>(SystemCall*)`, one loop over a chunk with
  `Unsafe.Add` over column base pointers (no bounds checks, no per-entity host calls).
- `Behaviour` subclasses: a nested `Data` component (one field per `[Property]`) and the
  partial property bodies (`AraData<Data>()`).
- `[Command]` classes: a `Params` struct and `__AraInvoke(CommandCall*)`.
- `AraModule.g.cs`: `[assembly: AraModule]` and `Register(ModuleBuilder)` with every schema
  type (types.json JSON from attributes and XML docs), layout, system access set,
  behaviour and command. With `AraNativeEntry=true` (and `<CompilerVisibleProperty>`), the
  NativeAOT `ara_script_init` export linking this and every referenced module, referenced
  modules first (dependency order) so registration sees the types each one uses.

## Rules

- Output depends only on declarations, attributes and doc comments: the compile server
  relies on that to keep body edits on L1. Keep it that way.
- Diagnostics ARA0701–0707 (`Descriptors.cs`): not partial, bad system parameter, missing
  doc, not blittable, bad command, bad behaviour, bad system method.
- Registration order is deterministic (sorted by schema name; systems by `(Order, name)`).
- `ColorPicker(Hdr = true, Temperature = true)` and `Header` combine into one schema
  `editor` object, keeping colour hints available to the native inspector and docs.

## Testing

`dotnet test tests/Ara.Generators.Tests`: generated registration compiles, body edits leave
output unchanged, the native entry, and each diagnostic.
